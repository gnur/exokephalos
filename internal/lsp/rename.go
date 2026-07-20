package lsp

import (
	"context"
	"fmt"
	"os"
	"strings"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/markdown"
	"go.lsp.dev/protocol"
	"go.lsp.dev/uri"
)

func PrepareRename(ctx context.Context, c *cache.Cache, text string, line, char int) (protocol.PrepareRenameResult, error) {
	lines := strings.Split(text, "\n")
	if line >= len(lines) {
		return nil, nil
	}

	link := WikilinkAtPosition(text, line, char)
	if link != nil && link.ID != "" {
		note := findNoteByID(c, link.ID)
		if note != nil {
			title := note.Title("title")
			result := protocol.PrepareRenameResult(&protocol.PrepareRenamePlaceholder{
				Range: protocol.Range{
					Start: protocol.Position{Line: uint32(link.Line), Character: uint32(link.StartCol)},
					End:   protocol.Position{Line: uint32(link.Line), Character: uint32(link.EndCol)},
				},
				Placeholder: title,
			})
			return result, nil
		}
		return nil, nil
	}

	currentLine := lines[line]
	trimmed := strings.TrimSpace(currentLine)
	if strings.HasPrefix(trimmed, "title:") {
		stripped := strings.TrimPrefix(trimmed, "title:")
		stripped = strings.TrimSpace(stripped)
		startCol := len(currentLine) - len(trimmed) + len("title: ")
		if strings.HasPrefix(trimmed, "title: ") {
			startCol = len(currentLine) - len(trimmed) + 7
		}
		endCol := startCol + len(stripped)
		result := protocol.PrepareRenameResult(&protocol.PrepareRenamePlaceholder{
			Range: protocol.Range{
				Start: protocol.Position{Line: uint32(line), Character: uint32(startCol)},
				End:   protocol.Position{Line: uint32(line), Character: uint32(endCol)},
			},
			Placeholder: stripped,
		})
		return result, nil
	}

	return nil, nil
}

func ReworkEdits(ctx context.Context, c *cache.Cache, text string, line, char int, newName string) (*protocol.WorkspaceEdit, error) {
	var targetPath string

	link := WikilinkAtPosition(text, line, char)
	if link != nil && link.ID != "" {
		note := findNoteByID(c, link.ID)
		if note == nil {
			return nil, fmt.Errorf("linked note not found: %s", link.ID)
		}
		targetPath = note.Path
	} else {
		lines := strings.Split(text, "\n")
		if line >= len(lines) {
			return nil, nil
		}
		currentLine := lines[line]
		trimmed := strings.TrimSpace(currentLine)
		if !strings.HasPrefix(trimmed, "title:") {
			return nil, fmt.Errorf("cursor not on a title field or wikilink")
		}
		fm, _, err := markdown.ParseFrontmatterBytes([]byte(text))
		if err != nil || fm == nil {
			return nil, fmt.Errorf("cannot parse frontmatter")
		}
		idVal := markdown.FMString(fm, "id")
		if idVal == "" {
			return nil, fmt.Errorf("no id in frontmatter")
		}
		note := findNoteByID(c, idVal)
		if note == nil {
			return nil, fmt.Errorf("note not found: %s", idVal)
		}
		targetPath = note.Path
	}

	content, err := os.ReadFile(targetPath)
	if err != nil {
		return nil, fmt.Errorf("reading target note: %w", err)
	}

	targetLines := strings.Split(string(content), "\n")

	var titleLineIdx = -1
	var titleLine string
	for i, tl := range targetLines {
		trimmed := strings.TrimSpace(tl)
		if strings.HasPrefix(trimmed, "title:") {
			titleLineIdx = i
			titleLine = trimmed
			break
		}
	}
	if titleLineIdx == -1 {
		return nil, fmt.Errorf("no title field in target note")
	}

	newLine := targetLines[titleLineIdx][:len(targetLines[titleLineIdx])-len(titleLine)] + "title: " + newName

	targetURI := pathToURI(targetPath)
	return &protocol.WorkspaceEdit{
		Changes: map[uri.URI][]protocol.TextEdit{
			targetURI: {
				{
					Range: protocol.Range{
						Start: protocol.Position{Line: uint32(titleLineIdx), Character: 0},
						End:   protocol.Position{Line: uint32(titleLineIdx + 1), Character: 0},
					},
					NewText: newLine + "\n",
				},
			},
		},
	}, nil
}
