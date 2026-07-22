package handlers

import (
	"testing"

	"github.com/gnur/exokephalos/internal/scanner"
)

func TestSortItemsUsesIDAsDateTieBreaker(t *testing.T) {
	items := []scanner.Item{
		{Path: "z.md", Frontmatter: map[string]interface{}{"created": "2026-07-08", "id": "zeta"}},
		{Path: "b.md", Frontmatter: map[string]interface{}{"created": "2026-07-09", "id": "beta"}},
		{Path: "a.md", Frontmatter: map[string]interface{}{"created": "2026-07-09", "id": "alpha"}},
	}

	sortItems(items, "created", "desc")

	got := []string{items[0].SortID(), items[1].SortID(), items[2].SortID()}
	want := []string{"alpha", "beta", "zeta"}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("order = %v, want %v", got, want)
		}
	}
}
