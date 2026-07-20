package lsp

import (
	"context"
	"fmt"
	"strings"
	"time"

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

	backlinks := 0
	if items, err := c.All(); err == nil {
		for _, candidate := range items {
			for _, candidateLink := range ParseWikilinks(candidate.Body) {
				if strings.EqualFold(candidateLink.ID, item.ID) || strings.EqualFold(candidateLink.ID, title) {
					backlinks++
				}
			}
		}
	}
	created := "unknown"
	if !item.Created.IsZero() {
		created = item.Created.Format(time.DateOnly)
	}
	content := fmt.Sprintf("## %s\n\n**Type:** %s  \n**Tags:** %s  \n**Created:** %s  \n**Backlinks:** %d\n\n---\n\n%s", title, item.Type, tags, created, backlinks, body)

	return &protocol.Hover{
		Contents: &protocol.MarkupContent{
			Kind:  protocol.MarkupKindMarkdown,
			Value: content,
		},
	}, nil
}
