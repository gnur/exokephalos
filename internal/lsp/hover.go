package lsp

import (
	"context"
	"fmt"
	"strings"

	"github.com/gnur/exokephalos/internal/cache"
	"go.lsp.dev/protocol"
)

func GetHover(ctx context.Context, c *cache.Cache, text string, line, char int) (*protocol.Hover, error) {
	link := WikilinkAtPosition(text, line, char)
	if link == nil {
		return nil, nil
	}

	item := findNoteByIDOrTitle(c, link.ID)
	if item == nil {
		return &protocol.Hover{
			Contents: &protocol.MarkupContent{
				Kind:  protocol.MarkupKindMarkdown,
				Value: fmt.Sprintf("*Note not found: `%s`*", link.ID),
			},
		}, nil
	}

	title := item.Title("title")
	tags := strings.Join(item.Tags, ", ")
	if tags == "" {
		tags = "_none_"
	}

	body := item.Body
	if len(body) > 300 {
		body = body[:300] + "..."
	}

	content := fmt.Sprintf("## %s\n\n**Tags:** %s\n\n---\n\n%s", title, tags, body)

	return &protocol.Hover{
		Contents: &protocol.MarkupContent{
			Kind:  protocol.MarkupKindMarkdown,
			Value: content,
		},
	}, nil
}
