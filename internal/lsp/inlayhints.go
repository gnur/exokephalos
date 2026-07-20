package lsp

import (
	"context"
	"strings"

	"github.com/gnur/exokephalos/internal/cache"
	"go.lsp.dev/protocol"
)

func GetInlayHints(ctx context.Context, c *cache.Cache, text string, startLine, endLine int) ([]protocol.InlayHint, error) {
	lines := strings.Split(text, "\n")
	if startLine < 0 {
		startLine = 0
	}
	if endLine >= len(lines) {
		endLine = len(lines) - 1
	}

	var hints []protocol.InlayHint

	for lineIdx := startLine; lineIdx <= endLine; lineIdx++ {
		line := lines[lineIdx]
		links := ParseWikilinks(line)

		for _, link := range links {
			if link.ID == "" {
				continue
			}

			note := findNoteByID(c, link.ID)
			if note == nil {
				continue
			}

			title := note.Title("title")
			if title == "" {
				continue
			}

			kind := protocol.InlayHintKindType
			paddingLeft := true
			hints = append(hints, protocol.InlayHint{
				Position: protocol.Position{
					Line:      uint32(lineIdx),
					Character: uint32(link.EndCol),
				},
				Label:       protocol.String(" " + title),
				Kind:        kind,
				PaddingLeft: &paddingLeft,
			})
		}
	}

	return hints, nil
}
