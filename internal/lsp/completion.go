package lsp

import (
	"context"
	"fmt"
	"strings"

	"github.com/gnur/exokephalos/internal/cache"
	"go.lsp.dev/protocol"
)

type CompletionContext int

const (
	CompletionContextNone CompletionContext = iota
	CompletionContextFrontmatterTags
	CompletionContextBodyTag
	CompletionContextWikilink
)

func DetectCompletionContext(text string, line, char int) (CompletionContext, string) {
	lines := strings.Split(text, "\n")
	if line >= len(lines) {
		return CompletionContextNone, ""
	}
	currentLine := lines[line]
	if char > len(currentLine) {
		char = len(currentLine)
	}
	prefix := currentLine[:char]

	if IsInsideWikilink(currentLine, char) {
		return CompletionContextWikilink, GetWikilinkPrefix(currentLine, char)
	}

	fmEnd := findFrontmatterEnd(text)
	if line <= fmEnd {
		if ctx, p := detectFrontmatterTagContext(lines, line, char); ctx != CompletionContextNone {
			return ctx, p
		}
	}

	if ctx, p := detectBodyTagContext(prefix); ctx != CompletionContextNone {
		return ctx, p
	}

	if ctx, p := detectHashTagContext(prefix); ctx != CompletionContextNone {
		return ctx, p
	}

	return CompletionContextNone, ""
}

func findFrontmatterEnd(text string) int {
	if !strings.HasPrefix(text, "---") {
		return -1
	}
	idx := strings.Index(text[3:], "---")
	if idx == -1 {
		return len(strings.Split(text, "\n"))
	}
	lines := strings.Split(text[:idx+3], "\n")
	return len(lines) - 1
}

func detectFrontmatterTagContext(lines []string, line, char int) (CompletionContext, string) {
	for i := line; i >= 0; i-- {
		trimmed := strings.TrimSpace(lines[i])
		if strings.HasPrefix(trimmed, "tags:") {
			rest := strings.TrimPrefix(trimmed, "tags:")
			rest = strings.TrimSpace(rest)

			if strings.HasPrefix(rest, "[") {
				prefix := ""
				if i == line {
					linePrefix := lines[line][:char]
					bracketIdx := strings.LastIndex(linePrefix, "[")
					if bracketIdx != -1 {
						afterBracket := linePrefix[bracketIdx+1:]
						commaIdx := strings.LastIndex(afterBracket, ",")
						if commaIdx != -1 {
							prefix = strings.TrimSpace(afterBracket[commaIdx+1:])
						} else {
							prefix = strings.TrimSpace(afterBracket)
						}
					}
				}
				return CompletionContextFrontmatterTags, prefix
			}

			if i < line {
				for j := i + 1; j <= line; j++ {
					itemLine := strings.TrimSpace(lines[j])
					if strings.HasPrefix(itemLine, "- ") {
						if j == line {
							prefix := strings.TrimPrefix(lines[line][:char], "- ")
							prefix = strings.TrimSpace(prefix)
							return CompletionContextFrontmatterTags, prefix
						}
					} else if !strings.HasPrefix(itemLine, "-") && itemLine != "" {
						break
					}
				}
			}
			break
		}
		if !strings.HasPrefix(trimmed, "-") && !strings.HasPrefix(trimmed, "tags") && trimmed != "" {
			break
		}
	}
	return CompletionContextNone, ""
}

func detectBodyTagContext(prefix string) (CompletionContext, string) {
	colonIdx := strings.LastIndex(prefix, ":")
	if colonIdx == -1 {
		return CompletionContextNone, ""
	}

	tagPrefix := prefix[colonIdx+1:]
	if strings.Contains(tagPrefix, " ") || strings.Contains(tagPrefix, "\n") {
		return CompletionContextNone, ""
	}

	if colonIdx > 0 {
		beforeColon := prefix[colonIdx-1 : colonIdx]
		if beforeColon != " " && beforeColon != "\t" && beforeColon != "\n" && beforeColon != "" {
			lastChar := prefix[colonIdx-1]
			if lastChar != ' ' && lastChar != '\t' && lastChar != '\n' && lastChar != '(' && lastChar != '[' {
				return CompletionContextNone, ""
			}
		}
	}

	return CompletionContextBodyTag, tagPrefix
}

func detectHashTagContext(prefix string) (CompletionContext, string) {
	hashIdx := strings.LastIndex(prefix, "#")
	if hashIdx == -1 {
		return CompletionContextNone, ""
	}

	if hashIdx > 0 {
		prevChar := prefix[hashIdx-1]
		if (prevChar >= 'a' && prevChar <= 'z') || (prevChar >= 'A' && prevChar <= 'Z') || (prevChar >= '0' && prevChar <= '9') {
			return CompletionContextNone, ""
		}
	}

	tagPrefix := prefix[hashIdx+1:]
	if strings.Contains(tagPrefix, " ") || strings.Contains(tagPrefix, "\n") {
		return CompletionContextNone, ""
	}

	return CompletionContextBodyTag, tagPrefix
}

func GetCompletions(ctx context.Context, c *cache.Cache, compCtx CompletionContext, prefix string) ([]protocol.CompletionItem, error) {
	switch compCtx {
	case CompletionContextWikilink:
		return getLinkCompletions(ctx, c, prefix)
	case CompletionContextFrontmatterTags, CompletionContextBodyTag:
		return getTagCompletions(ctx, c, prefix)
	default:
		return nil, nil
	}
}

func getLinkCompletions(ctx context.Context, c *cache.Cache, prefix string) ([]protocol.CompletionItem, error) {
	items, err := c.All()
	if err != nil {
		return nil, err
	}

	prefix = strings.ToLower(prefix)
	var completions []protocol.CompletionItem
	seen := make(map[string]bool)

	for _, item := range items {
		if item.ID == "" {
			continue
		}
		if item.Type != "note" && item.Type != "book" {
			continue
		}

		idLower := strings.ToLower(item.ID)
		title := item.Title("title")
		titleLower := strings.ToLower(title)

		if strings.Contains(idLower, prefix) || strings.Contains(titleLower, prefix) {
			if seen[item.ID] {
				continue
			}
			seen[item.ID] = true

			kind := protocol.CompletionItemKindReference
			titleStr := title
			completions = append(completions, protocol.CompletionItem{
				Label:      title,
				FilterText: protocol.NewOptional(titleStr),
				InsertText: protocol.NewOptional(item.ID),
				Kind:       kind,
				Detail:     protocol.NewOptional(item.ID),
			})
		}
	}

	return completions, nil
}

func getTagCompletions(ctx context.Context, c *cache.Cache, prefix string) ([]protocol.CompletionItem, error) {
	items, err := c.All()
	if err != nil {
		return nil, err
	}

	tagCounts := make(map[string]int)
	for _, item := range items {
		for _, tag := range item.Tags {
			tagCounts[tag]++
		}
	}

	prefix = strings.ToLower(prefix)
	var completions []protocol.CompletionItem

	for tag, count := range tagCounts {
		if strings.HasPrefix(strings.ToLower(tag), prefix) {
			kind := protocol.CompletionItemKindKeyword
			detail := formatTagCount(count)
			completions = append(completions, protocol.CompletionItem{
				Label:  tag,
				Kind:   kind,
				Detail: protocol.NewOptional(detail),
			})
		}
	}

	return completions, nil
}

func formatTagCount(count int) string {
	if count == 1 {
		return "1 note"
	}
	return fmt.Sprintf("%d notes", count)
}
