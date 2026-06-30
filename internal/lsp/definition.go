package lsp

import (
	"context"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/modern-dev/go-lsp/protocol"
)

func GetDefinition(ctx context.Context, c *cache.Cache, text string, line, char int) (*protocol.Location, error) {
	link := WikilinkAtPosition(text, line, char)
	if link == nil {
		return nil, nil
	}

	item := findNoteByIDOrTitle(c, link.ID)
	if item == nil {
		return nil, nil
	}

	uri := pathToURI(item.Path)
	return &protocol.Location{
		URI: uri,
		Range: protocol.Range{
			Start: protocol.Position{Line: 0, Character: 0},
			End:   protocol.Position{Line: 0, Character: 0},
		},
	}, nil
}
