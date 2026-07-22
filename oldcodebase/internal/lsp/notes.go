package lsp

import (
	"strings"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/scanner"
)

// findNoteByID searches the cache for an item with the exact matching ID (case-insensitive).
func findNoteByID(c *cache.Cache, id string) *scanner.Item {
	if item, err := c.GetByID(id); err == nil {
		return item
	}
	return nil
}

// findNoteByIDOrTitle searches the cache for an item by matching ID or title,
// prioritizing exact ID, exact title, prefix ID, and finally substring title (case-insensitive).
func findNoteByIDOrTitle(c *cache.Cache, query string) *scanner.Item {
	// 1. Try exact ID lookup (O(1))
	if item, err := c.GetByID(query); err == nil {
		return item
	}

	items, err := c.All()
	if err != nil {
		return nil
	}

	queryLower := strings.ToLower(query)

	// Since exact ID check was already done above, we proceed to title and prefix matches
	for i := range items {
		title := items[i].Title("title")
		if strings.ToLower(title) == queryLower {
			return &items[i]
		}
	}

	for i := range items {
		if strings.HasPrefix(strings.ToLower(items[i].ID), queryLower) {
			return &items[i]
		}
	}

	for i := range items {
		title := items[i].Title("title")
		if strings.Contains(strings.ToLower(title), queryLower) {
			return &items[i]
		}
	}

	return nil
}
