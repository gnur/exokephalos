package lsp

import (
	"context"
	"strings"
	"slices"

	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/modern-dev/go-lsp/protocol"
)

func GetCodeActions(ctx context.Context, text string, line int, uri protocol.DocumentURI) ([]protocol.CodeAction, error) {
	lines := strings.Split(text, "\n")
	if line >= len(lines) {
		return nil, nil
	}

	currentLine := lines[line]
	var actions []protocol.CodeAction

	if isListItem(currentLine) {
		action := createStrikethroughAction(currentLine, line, uri)
		if action != nil {
			actions = append(actions, *action)
		}
	}

	fm, _, err := markdown.ParseFrontmatterBytes([]byte(text))
	if err == nil && fm != nil {
		docType, _ := fm["type"].(string)
		tags := markdown.ExtractTags(fm)

		if docType == "note" {
			hasTodo := containsTag(tags, "todo")
			hasDone := containsTag(tags, "done")
			if hasTodo && !hasDone {
				action := createMarkDoneAction(text, uri)
				if action != nil {
					actions = append(actions, *action)
				}
			}
		}

		if docType == "book" {
			bookActions := createBookStatusActions(text, uri, tags)
			actions = append(actions, bookActions...)
		}
	}

	return actions, nil
}


func containsTag(tags []string, tag string) bool {
	return slices.Contains(tags, tag)
}

func createMarkDoneAction(text string, uri protocol.DocumentURI) *protocol.CodeAction {
	lines := strings.Split(text, "\n")
	var tagsLineIdx int = -1
	var tagsContent string
	for i, line := range lines {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "tags:") {
			tagsLineIdx = i
			tagsContent = trimmed
			break
		}
	}
	if tagsLineIdx == -1 {
		return nil
	}

	newTagsContent := addTagToLine(tagsContent, "done")
	newLine := lines[tagsLineIdx][:len(lines[tagsLineIdx])-len(tagsContent)] + newTagsContent

	title := "Mark as done"
	kind := protocol.CodeActionKindQuickFix
	data := any(map[string]any{
		"uri":        string(uri),
		"actionType": "markDone",
		"tagLineIdx": tagsLineIdx,
		"newLine":    newLine,
	})

	return &protocol.CodeAction{
		Title: title,
		Kind:  &kind,
		Data:  &data,
	}
}

func createBookStatusActions(text string, uri protocol.DocumentURI, tags []string) []protocol.CodeAction {
	var actions []protocol.CodeAction
	statuses := []string{"to-read", "reading", "read"}

	currentStatus := ""
	for _, s := range statuses {
		if containsTag(tags, s) {
			currentStatus = s
			break
		}
	}

	lines := strings.Split(text, "\n")
	var tagsLineIdx int = -1
	var tagsContent string
	for i, line := range lines {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "tags:") {
			tagsLineIdx = i
			tagsContent = trimmed
			break
		}
	}
	if tagsLineIdx == -1 {
		return nil
	}

	for _, status := range statuses {
		if status == currentStatus {
			continue
		}
		newTagsContent := tagsContent
		for _, s := range statuses {
			if s == currentStatus {
				newTagsContent = removeTagFromLine(newTagsContent, s)
			}
		}
		newTagsContent = addTagToLine(newTagsContent, status)
		newLine := lines[tagsLineIdx][:len(lines[tagsLineIdx])-len(tagsContent)] + newTagsContent

		title := "Mark as " + status
		kind := protocol.CodeActionKindQuickFix
		data := any(map[string]any{
			"uri":        string(uri),
			"actionType": "bookStatus",
			"tagLineIdx": tagsLineIdx,
			"newLine":    newLine,
		})
		actions = append(actions, protocol.CodeAction{
			Title: title,
			Kind:  &kind,
			Data:  &data,
		})
	}

	return actions
}

func isListItem(line string) bool {
	trimmed := strings.TrimSpace(line)
	if strings.HasPrefix(trimmed, "- ") || strings.HasPrefix(trimmed, "* ") {
		return true
	}
	if len(trimmed) >= 3 && trimmed[0] >= '0' && trimmed[0] <= '9' {
		dotIdx := strings.Index(trimmed, ". ")
		if dotIdx > 0 && dotIdx < 4 {
			return true
		}
	}
	return false
}

func createStrikethroughAction(line string, lineNum int, uri protocol.DocumentURI) *protocol.CodeAction {
	trimmed := strings.TrimSpace(line)
	indent := line[:len(line)-len(trimmed)]

	var bullet, content string
	if strings.HasPrefix(trimmed, "- ") {
		bullet = "- "
		content = trimmed[2:]
	} else if strings.HasPrefix(trimmed, "* ") {
		bullet = "* "
		content = trimmed[2:]
	} else {
		dotIdx := strings.Index(trimmed, ". ")
		if dotIdx > 0 {
			bullet = trimmed[:dotIdx+2]
			content = trimmed[dotIdx+2:]
		}
	}

	if content == "" {
		return nil
	}

	var newContent string
	if strings.HasPrefix(content, "~~") && strings.HasSuffix(content, "~~") {
		newContent = content[2 : len(content)-2]
	} else {
		newContent = "~~" + content + "~~"
	}

	newLine := indent + bullet + newContent

	title := "Toggle strikethrough"
	kind := protocol.CodeActionKindRefactor
	data := any(map[string]any{
		"uri":     string(uri),
		"line":    lineNum,
		"newLine": newLine,
	})

	return &protocol.CodeAction{
		Title: title,
		Kind:  &kind,
		Data:  &data,
	}
}

func ResolveCodeAction(ctx context.Context, action *protocol.CodeAction) (*protocol.CodeAction, error) {
	if action.Data == nil {
		return action, nil
	}

	dataMap, ok := (*action.Data).(map[string]any)
	if !ok {
		return action, nil
	}

	uriStr, ok := dataMap["uri"].(string)
	if !ok {
		return action, nil
	}
	uri := protocol.DocumentURI(uriStr)

	actionType, _ := dataMap["actionType"].(string)

	switch actionType {
	case "markDone":
		return resolveMarkDone(action, uri, dataMap)
	case "bookStatus":
		return resolveBookStatus(action, uri, dataMap)
	default:
		return resolveStrikethrough(action, uri, dataMap)
	}
}

func resolveStrikethrough(action *protocol.CodeAction, uri protocol.DocumentURI, dataMap map[string]any) (*protocol.CodeAction, error) {
	lineNum, ok := dataMap["line"].(float64)
	if !ok {
		return action, nil
	}
	newLine, ok := dataMap["newLine"].(string)
	if !ok {
		return action, nil
	}

	line := int(lineNum)
	action.Edit = &protocol.WorkspaceEdit{
		Changes: map[protocol.DocumentURI][]protocol.TextEdit{
			uri: {
				{
					Range: protocol.Range{
						Start: protocol.Position{Line: uint32(line), Character: 0},
						End:   protocol.Position{Line: uint32(line + 1), Character: 0},
					},
					NewText: newLine + "\n",
				},
			},
		},
	}

	return action, nil
}

func resolveMarkDone(action *protocol.CodeAction, uri protocol.DocumentURI, dataMap map[string]any) (*protocol.CodeAction, error) {
	tagLineIdx, ok := dataMap["tagLineIdx"].(float64)
	if !ok {
		return action, nil
	}
	newLine, ok := dataMap["newLine"].(string)
	if !ok {
		return action, nil
	}

	line := int(tagLineIdx)
	action.Edit = &protocol.WorkspaceEdit{
		Changes: map[protocol.DocumentURI][]protocol.TextEdit{
			uri: {
				{
					Range: protocol.Range{
						Start: protocol.Position{Line: uint32(line), Character: 0},
						End:   protocol.Position{Line: uint32(line + 1), Character: 0},
					},
					NewText: newLine + "\n",
				},
			},
		},
	}

	return action, nil
}

func resolveBookStatus(action *protocol.CodeAction, uri protocol.DocumentURI, dataMap map[string]any) (*protocol.CodeAction, error) {
	tagLineIdx, ok := dataMap["tagLineIdx"].(float64)
	if !ok {
		return action, nil
	}
	newLine, ok := dataMap["newLine"].(string)
	if !ok {
		return action, nil
	}

	line := int(tagLineIdx)
	action.Edit = &protocol.WorkspaceEdit{
		Changes: map[protocol.DocumentURI][]protocol.TextEdit{
			uri: {
				{
					Range: protocol.Range{
						Start: protocol.Position{Line: uint32(line), Character: 0},
						End:   protocol.Position{Line: uint32(line + 1), Character: 0},
					},
					NewText: newLine + "\n",
				},
			},
		},
	}

	return action, nil
}

func addTagToLine(line string, tag string) string {
	if strings.Contains(line, "["+tag+"]") || strings.Contains(line, ", "+tag) || strings.Contains(line, ","+tag) {
		return line
	}

	if strings.Contains(line, "[") {
		if strings.HasSuffix(strings.TrimSpace(line), "]") {
			trimmed := strings.TrimSpace(line)
			if strings.HasSuffix(trimmed, "[]") {
				return strings.TrimSuffix(line, "]") + tag + "]"
			}
			return strings.TrimSuffix(line, "]") + ", " + tag + "]"
		}
		return line + ", " + tag
	}

	return line + "\n  - " + tag
}

func removeTagFromLine(line string, tag string) string {
	line = strings.ReplaceAll(line, ", "+tag, "")
	line = strings.ReplaceAll(line, ","+tag, "")
	line = strings.ReplaceAll(line, "["+tag+", ", "[")
	line = strings.ReplaceAll(line, "["+tag+"]", "[]")
	line = strings.ReplaceAll(line, "  - "+tag+"\n", "")
	line = strings.ReplaceAll(line, "  - "+tag, "")
	return line
}
