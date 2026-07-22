package lsp

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/gnur/exokephalos/internal/id"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/scanner"
	"go.lsp.dev/protocol"
	"go.lsp.dev/uri"
)

func (s *Server) publishDiagnostics(uri uri.URI, text string) {
	diagnostics := []protocol.Diagnostic{} // Initialize as empty slice, not nil

	if text != "" {
		links := ParseWikilinks(text)
		severity := protocol.DiagnosticSeverityWarning
		source := "exokephalos"

		for _, link := range links {
			if link.ID == "" {
				diagnostics = append(diagnostics, diagnosticAt(link.Line, link.StartCol, link.EndCol, "Empty wikilink"))
				continue
			}

			note := findNoteByIDOrTitle(s.cache, link.ID)
			if note != nil {
				continue
			}

			diagnostics = append(diagnostics, protocol.Diagnostic{Range: protocol.Range{Start: protocol.Position{Line: uint32(link.Line), Character: uint32(link.StartCol)}, End: protocol.Position{Line: uint32(link.Line), Character: uint32(link.EndCol)}}, Severity: severity, Source: protocol.NewOptional(source), Message: protocol.String("Note not found: " + link.ID)})
		}
		for line, value := range strings.Split(text, "\n") {
			for _, link := range markdownLinkRanges(value) {
				if strings.Contains(link.target, "://") || strings.HasPrefix(link.target, "#") {
					continue
				}
				path := filepath.Join(filepath.Dir(uri.Path()), link.target)
				if _, err := os.Stat(path); err != nil {
					diagnostics = append(diagnostics, diagnosticAt(line, link.start, link.end, "Markdown link target not found: "+link.target))
				}
			}
		}

		fm, _, err := markdown.ParseFrontmatterBytes([]byte(text))
		if err != nil {
			diagnostics = append(diagnostics, diagnosticAt(0, 0, 0, "Invalid YAML frontmatter: "+err.Error()))
		} else {
			for _, field := range []string{"id", "type", "tags"} {
				if fm == nil || strings.TrimSpace(markdown.FMString(fm, field)) == "" && field != "tags" {
					diagnostics = append(diagnostics, diagnosticAt(0, 0, 0, "Missing required frontmatter field: "+field))
				}
			}
			if fm != nil {
				if value := markdown.FMString(fm, "id"); value != "" && !id.IsValidID(value) {
					diagnostics = append(diagnostics, diagnosticAt(0, 0, 0, "Invalid note ID: "+value))
				}
				if _, ok := fm["created"]; !ok {
					if _, added := fm["added"]; !added {
						diagnostics = append(diagnostics, diagnosticAt(0, 0, 0, "Missing required frontmatter field: created"))
					}
				}
				if _, ok := fm["tags"]; !ok {
					diagnostics = append(diagnostics, diagnosticAt(0, 0, 0, "Missing required frontmatter field: tags"))
				}
			}
		}
		if all, err := scanner.ScanAll(s.baseDir); err == nil {
			counts := map[string]int{}
			for _, item := range all {
				if item.ID != "" {
					counts[strings.ToLower(item.ID)]++
				}
			}
			if fm != nil && counts[strings.ToLower(markdown.FMString(fm, "id"))] > 1 {
				diagnostics = append(diagnostics, diagnosticAt(0, 0, 0, fmt.Sprintf("Duplicate note ID: %s", markdown.FMString(fm, "id"))))
			}
		}
	}

	s.client.PublishDiagnostics(context.Background(), &protocol.PublishDiagnosticsParams{
		URI:         uri,
		Diagnostics: diagnostics,
	})
}

func diagnosticAt(line, start, end int, message string) protocol.Diagnostic {
	severity := protocol.DiagnosticSeverityWarning
	return protocol.Diagnostic{Range: protocol.Range{Start: protocol.Position{Line: uint32(line), Character: uint32(start)}, End: protocol.Position{Line: uint32(line), Character: uint32(end)}}, Severity: severity, Source: protocol.NewOptional("exokephalos"), Message: protocol.String(message)}
}
