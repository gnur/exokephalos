package itemcreate

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestNewBuildsVerifiedStandardItem(t *testing.T) {
	item, err := New(t.TempDir(), "book", "A New Book", "body")
	if err != nil {
		t.Fatal(err)
	}
	if err := Verify(item.Frontmatter, "book", "A New Book"); err != nil {
		t.Fatal(err)
	}
	if item.Frontmatter["id"] == "" || item.Frontmatter["created"] == "" {
		t.Fatalf("generated fields missing: %#v", item.Frontmatter)
	}
	if tags, ok := item.Frontmatter["tags"].([]interface{}); !ok || len(tags) != 0 {
		t.Fatalf("tags = %#v", item.Frontmatter["tags"])
	}
	if !strings.Contains(item.Path, string(filepath.Separator)+"book"+string(filepath.Separator)) || !strings.HasSuffix(item.Path, "a-new-book.md") {
		t.Fatalf("path = %q", item.Path)
	}
}

func TestNewRequiresTypeAndTitle(t *testing.T) {
	for _, input := range [][2]string{{"", "Title"}, {"note", ""}, {"../note", "Title"}} {
		if _, err := New(t.TempDir(), input[0], input[1], ""); err == nil {
			t.Fatalf("New(%q, %q) succeeded", input[0], input[1])
		}
	}
}

func TestWriteCreatesDestinationAndVerifiesFrontmatter(t *testing.T) {
	item, err := New(t.TempDir(), "note", "Written Note", "body")
	if err != nil {
		t.Fatal(err)
	}
	if err := Write(item); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(item.Path); err != nil {
		t.Fatal(err)
	}
}
