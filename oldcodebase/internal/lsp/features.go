package lsp

import (
	"context"
	"fmt"
	"path/filepath"
	"strings"

	"github.com/gnur/exokephalos/internal/id"
	"github.com/gnur/exokephalos/internal/itemcreate"
	"github.com/gnur/exokephalos/internal/markdown"
	"go.lsp.dev/protocol"
	"go.lsp.dev/uri"
)

func (s *Server) missingNoteAction(text string, line, character int) *protocol.CodeAction {
	link := WikilinkAtPosition(text, line, character)
	if link == nil || link.ID == "" || findNoteByIDOrTitle(s.cache, link.ID) != nil {
		return nil
	}
	data, err := protocol.Marshal(map[string]string{"actionType": "createMissingNote", "title": link.ID})
	if err != nil {
		return nil
	}
	kind := protocol.CodeActionKindQuickFix
	return &protocol.CodeAction{Title: fmt.Sprintf("Create note %q", link.ID), Kind: &kind, Data: data}
}

func (s *Server) resolveCodeAction(ctx context.Context, action *protocol.CodeAction) (*protocol.CodeAction, error) {
	if action.Data != nil {
		var data map[string]string
		if protocol.Unmarshal(action.Data, &data) == nil {
			switch data["actionType"] {
			case "createMissingNote":
				if err := s.createNote(data["title"]); err != nil {
					return action, err
				}
				return action, nil
			case "normalizeFrontmatter":
				return s.normalizeFrontmatterAction(action, data)
			}
		}
	}
	return ResolveCodeAction(ctx, action)
}

func (s *Server) frontmatterAction(text string, documentURI uri.URI) *protocol.CodeAction {
	fm, _, err := markdown.ParseFrontmatterBytes([]byte(text))
	if err == nil && fm != nil && markdown.FMString(fm, "id") != "" && markdown.FMString(fm, "type") != "" {
		if _, tags := fm["tags"]; tags {
			if _, created := fm["created"]; created {
				return nil
			}
		}
	}
	data, err := protocol.Marshal(map[string]string{"actionType": "normalizeFrontmatter", "uri": documentURI.String(), "content": text})
	if err != nil {
		return nil
	}
	kind := protocol.CodeActionKindQuickFix
	return &protocol.CodeAction{Title: "Normalize required frontmatter", Kind: &kind, Data: data}
}

func (s *Server) normalizeFrontmatterAction(action *protocol.CodeAction, data map[string]string) (*protocol.CodeAction, error) {
	viewID := "note"
	if s.config != nil && len(s.config.OrderedViews()) > 0 {
		viewID = s.config.OrderedViews()[s.config.DefaultViewIndex()].ID
	}
	content, err := markdown.EnsureRequiredFields(data["content"], id.GenerateID(), strings.TrimSuffix(viewID, "s"))
	if err != nil {
		return action, err
	}
	documentURI := uri.URI(data["uri"])
	lineCount := len(strings.Split(data["content"], "\n"))
	action.Edit = &protocol.WorkspaceEdit{Changes: map[uri.URI][]protocol.TextEdit{documentURI: {{Range: protocol.Range{Start: protocol.Position{}, End: protocol.Position{Line: uint32(lineCount), Character: 0}}, NewText: content}}}}
	return action, nil
}

func (s *Server) createNote(title string) error {
	item, err := itemcreate.New(s.baseDir, "note", title, "")
	if err != nil {
		return err
	}
	if err := itemcreate.Verify(item.Frontmatter, "note", strings.TrimSpace(title)); err != nil {
		return err
	}
	return s.repo.CreateItem(item.Path, item.Frontmatter, item.Body)
}

func documentSymbols(text string) []protocol.DocumentSymbol {
	lines := strings.Split(text, "\n")
	symbols := make([]protocol.DocumentSymbol, 0)
	inFrontmatter := false
	for line, value := range lines {
		trimmed := strings.TrimSpace(value)
		if line == 0 && trimmed == "---" {
			inFrontmatter = true
			continue
		}
		if inFrontmatter && trimmed == "---" {
			inFrontmatter = false
			continue
		}
		if inFrontmatter {
			if colon := strings.Index(trimmed, ":"); colon > 0 {
				name := strings.TrimSpace(trimmed[:colon])
				symbols = append(symbols, symbol(name, protocol.SymbolKindProperty, line, 0, len(value)))
			}
			continue
		}
		if strings.HasPrefix(trimmed, "#") {
			name := strings.TrimSpace(strings.TrimLeft(trimmed, "#"))
			if name != "" {
				indent := strings.Index(value, "#")
				symbols = append(symbols, symbol(name, protocol.SymbolKindString, line, indent, len(value)))
			}
		}
	}
	return symbols
}

func symbol(name string, kind protocol.SymbolKind, line, start, end int) protocol.DocumentSymbol {
	r := protocol.Range{Start: protocol.Position{Line: uint32(line), Character: uint32(start)}, End: protocol.Position{Line: uint32(line), Character: uint32(end)}}
	return protocol.DocumentSymbol{Name: name, Kind: kind, Range: r, SelectionRange: r}
}

func (s *Server) documentLinks(text string, documentURI uri.URI) []protocol.DocumentLink {
	links := make([]protocol.DocumentLink, 0)
	for _, link := range ParseWikilinks(text) {
		item := findNoteByIDOrTitle(s.cache, link.ID)
		if item == nil {
			continue
		}
		target := pathToURI(item.Path)
		links = append(links, protocol.DocumentLink{
			Range:  protocol.Range{Start: protocol.Position{Line: uint32(link.Line), Character: uint32(link.StartCol)}, End: protocol.Position{Line: uint32(link.Line), Character: uint32(link.EndCol)}},
			Target: &target,
		})
	}
	for line, value := range strings.Split(text, "\n") {
		for _, match := range markdownLinkRanges(value) {
			targetPath := filepath.Clean(filepath.Join(filepath.Dir(documentURI.Path()), match.target))
			if !strings.HasSuffix(strings.ToLower(targetPath), ".md") {
				continue
			}
			target := uri.File(targetPath)
			links = append(links, protocol.DocumentLink{
				Range:  protocol.Range{Start: protocol.Position{Line: uint32(line), Character: uint32(match.start)}, End: protocol.Position{Line: uint32(line), Character: uint32(match.end)}},
				Target: &target,
			})
		}
	}
	return links
}

type markdownLinkRange struct {
	start, end int
	target     string
}

func markdownLinkRanges(line string) []markdownLinkRange {
	var links []markdownLinkRange
	for offset := 0; offset < len(line); {
		open := strings.Index(line[offset:], "](")
		if open < 0 {
			break
		}
		open += offset
		close := strings.IndexByte(line[open+2:], ')')
		if close < 0 {
			break
		}
		close += open + 2
		start := strings.LastIndex(line[:open], "[")
		if start >= 0 && close > open+2 {
			links = append(links, markdownLinkRange{start: start, end: close + 1, target: line[open+2 : close]})
		}
		offset = close + 1
	}
	return links
}

func (s *Server) workspaceSymbols(query string) (protocol.WorkspaceSymbolResult, error) {
	items, err := s.cache.All()
	if err != nil {
		return nil, err
	}
	query = strings.ToLower(query)
	result := make(protocol.WorkspaceSymbolSlice, 0)
	for _, item := range items {
		name := item.Title("title")
		haystack := strings.ToLower(strings.Join(append([]string{name, item.ID, item.Type}, item.Tags...), " "))
		if query != "" && !strings.Contains(haystack, query) {
			continue
		}
		location := &protocol.Location{URI: pathToURI(item.Path), Range: protocol.Range{Start: protocol.Position{}, End: protocol.Position{}}}
		result = append(result, protocol.WorkspaceSymbol{
			BaseSymbolInformation: protocol.BaseSymbolInformation{Name: name, Kind: protocol.SymbolKindFile, ContainerName: &item.Type},
			Location:              location,
		})
	}
	return result, nil
}
