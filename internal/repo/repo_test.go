package repo

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/markdown"
)

func TestUpdateItem_PreservesBody(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "repo-test-*")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(tmpDir)

	bookDir := filepath.Join(tmpDir, "books", "to-read")
	if err := os.MkdirAll(bookDir, 0755); err != nil {
		t.Fatal(err)
	}

	bookPath := filepath.Join(bookDir, "test-book.md")
	fm := map[string]interface{}{
		"type":   "book",
		"title":  "Test Book",
		"author": []string{"Test Author"},
		"pages":  100,
	}
	body := "This is the book body content that should be preserved."
	if err := markdown.WriteFrontmatter(bookPath, fm, body); err != nil {
		t.Fatal(err)
	}

	c, err := cache.New(tmpDir)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()

	r := New(tmpDir, c)

	updatedFM := map[string]interface{}{
		"type":   "book",
		"title":  "Updated Title",
		"author": []string{"New Author"},
		"pages":  200,
	}
	if err := r.UpdateItem(bookPath, updatedFM, body); err != nil {
		t.Fatalf("UpdateItem failed: %v", err)
	}

	resultFM, resultBody, err := markdown.ParseFrontmatter(bookPath)
	if err != nil {
		t.Fatalf("ParseFrontmatter failed: %v", err)
	}

	if !strings.Contains(resultBody, body) {
		t.Errorf("Body content not preserved: got %q, want to contain %q", resultBody, body)
	}

	if resultFM["title"] != "Updated Title" {
		t.Errorf("Title not updated: got %v, want %q", resultFM["title"], "Updated Title")
	}
}

func TestCreateItem(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "repo-test-*")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(tmpDir)

	c, err := cache.New(tmpDir)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()

	r := New(tmpDir, c)

	itemPath := filepath.Join(tmpDir, "books", "to-read", "new-book.md")
	fm := map[string]interface{}{
		"type":   "book",
		"title":  "New Book",
		"author": []string{"New Author"},
		"pages":  150,
	}
	body := "This is the new book body."

	if err := r.CreateItem(itemPath, fm, body); err != nil {
		t.Fatalf("CreateItem failed: %v", err)
	}

	resultFM, resultBody, err := markdown.ParseFrontmatter(itemPath)
	if err != nil {
		t.Fatalf("ParseFrontmatter failed: %v", err)
	}

	if resultFM["title"] != "New Book" {
		t.Errorf("Title not set: got %v, want %q", resultFM["title"], "New Book")
	}

	if !strings.Contains(resultBody, body) {
		t.Errorf("Body not set: got %q, want to contain %q", resultBody, body)
	}
}
