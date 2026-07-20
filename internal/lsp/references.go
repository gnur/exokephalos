package lsp

import (
	"context"
	"strings"

	"github.com/gnur/exokephalos/internal/cache"
	"go.lsp.dev/protocol"
)

func GetReferences(ctx context.Context, c *cache.Cache, text string, line, char int) ([]protocol.Location, error) {
	link := WikilinkAtPosition(text, line, char)
	if link == nil || link.ID == "" {
		return nil, nil
	}

	target := findNoteByIDOrTitle(c, link.ID)
	if target == nil {
		return nil, nil
	}

	items, err := c.All()
	if err != nil {
		return nil, err
	}

	var locations []protocol.Location

	for _, item := range items {
		itemLinks := ParseWikilinks(item.Body)
		for _, l := range itemLinks {
			linked := findNoteByIDOrTitle(c, l.ID)
			if linked != nil && strings.EqualFold(linked.ID, target.ID) {
				locations = append(locations, protocol.Location{
					URI: pathToURI(item.Path),
					Range: protocol.Range{
						Start: protocol.Position{Line: uint32(l.Line), Character: uint32(l.StartCol)},
						End:   protocol.Position{Line: uint32(l.Line), Character: uint32(l.EndCol)},
					},
				})
			}
		}
	}

	return locations, nil
}
