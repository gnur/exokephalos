package lsp

import (
	"context"

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
				continue
			}

			note := findNoteByID(s.cache, link.ID)
			if note != nil {
				continue
			}

			diagnostics = append(diagnostics, protocol.Diagnostic{
				Range: protocol.Range{
					Start: protocol.Position{Line: uint32(link.Line), Character: uint32(link.StartCol)},
					End:   protocol.Position{Line: uint32(link.Line), Character: uint32(link.EndCol)},
				},
				Severity: severity,
				Source:   protocol.NewOptional(source),
				Message:  protocol.String("Note not found: " + link.ID),
			})
		}
	}

	s.client.PublishDiagnostics(context.Background(), &protocol.PublishDiagnosticsParams{
		URI:         uri,
		Diagnostics: diagnostics,
	})
}
