package scanner

import (
	"os"
	"path/filepath"
	"testing"
)

func writeFile(t *testing.T, path, content string) {
	t.Helper()
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		t.Fatalf("mkdir %s: %v", dir, err)
	}
	if err := os.WriteFile(path, []byte(content), 0644); err != nil {
		t.Fatalf("write %s: %v", path, err)
	}
}

func TestScanAll_ReadsMDFiles(t *testing.T) {
	dir := t.TempDir()
	writeFile(t, filepath.Join(dir, "note1.md"), "---\ntype: note\ntitle: First\n---\nBody one\n")
	writeFile(t, filepath.Join(dir, "note2.md"), "---\ntype: note\ntitle: Second\n---\nBody two\n")

	items, err := ScanAll(dir)
	if err != nil {
		t.Fatalf("ScanAll: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 items, got %d", len(items))
	}
}

func TestScanAll_SkipsHiddenDirs(t *testing.T) {
	dir := t.TempDir()
	writeFile(t, filepath.Join(dir, "visible.md"), "---\ntype: note\n---\n")
	writeFile(t, filepath.Join(dir, ".git", "config.md"), "---\ntype: hidden\n---\n")
	writeFile(t, filepath.Join(dir, ".zk", "index.md"), "---\ntype: hidden\n---\n")
	writeFile(t, filepath.Join(dir, ".obsidian", "workspace.md"), "---\ntype: hidden\n---\n")

	items, err := ScanAll(dir)
	if err != nil {
		t.Fatalf("ScanAll: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 item (hidden dirs skipped), got %d", len(items))
	}
	if filepath.Base(items[0].Path) != "visible.md" {
		t.Errorf("expected visible.md, got %s", filepath.Base(items[0].Path))
	}
}

func TestScanAll_SkipsNonMDFiles(t *testing.T) {
	dir := t.TempDir()
	writeFile(t, filepath.Join(dir, "note.md"), "---\ntype: note\n---\n")
	writeFile(t, filepath.Join(dir, "readme.txt"), "plain text")
	writeFile(t, filepath.Join(dir, "data.json"), "{}")
	writeFile(t, filepath.Join(dir, "script.sh"), "#!/bin/bash")

	items, err := ScanAll(dir)
	if err != nil {
		t.Fatalf("ScanAll: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 item (.md only), got %d", len(items))
	}
}

func TestScanAll_ParsesFrontmatter(t *testing.T) {
	dir := t.TempDir()
	writeFile(t, filepath.Join(dir, "test.md"), "---\ntype: book\ntitle: My Book\nauthor: Jane\ntags:\n  - fiction\n  - sci-fi\n---\nContent here\n")

	items, err := ScanAll(dir)
	if err != nil {
		t.Fatalf("ScanAll: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 item, got %d", len(items))
	}

	item := items[0]
	if item.Frontmatter["type"] != "book" {
		t.Errorf("type = %v, want 'book'", item.Frontmatter["type"])
	}
	if item.Frontmatter["title"] != "My Book" {
		t.Errorf("title = %v, want 'My Book'", item.Frontmatter["title"])
	}
	if item.Frontmatter["author"] != "Jane" {
		t.Errorf("author = %v, want 'Jane'", item.Frontmatter["author"])
	}
	if item.Body != "Content here\n" {
		t.Errorf("body = %q, want %q", item.Body, "Content here\n")
	}
}

func TestScanAll_SubdirectoryFiles(t *testing.T) {
	dir := t.TempDir()
	writeFile(t, filepath.Join(dir, "notes", "one.md"), "---\ntype: note\n---\n")
	writeFile(t, filepath.Join(dir, "books", "two.md"), "---\ntype: book\n---\n")

	items, err := ScanAll(dir)
	if err != nil {
		t.Fatalf("ScanAll: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 items from subdirs, got %d", len(items))
	}
}

func TestItem_Title_WithField(t *testing.T) {
	item := &Item{
		Path:        "/tmp/test.md",
		Frontmatter: map[string]interface{}{"title": "My Title"},
	}
	if got := item.Title("title"); got != "My Title" {
		t.Errorf("Title() = %q, want %q", got, "My Title")
	}
}

func TestItem_Title_FallbackToFilename(t *testing.T) {
	item := &Item{
		Path:        "/tmp/some-note.md",
		Frontmatter: map[string]interface{}{"type": "note"},
	}
	if got := item.Title("title"); got != "some-note" {
		t.Errorf("Title() = %q, want %q", got, "some-note")
	}
}

func TestItem_Title_EmptyFieldDefaultsToTitle(t *testing.T) {
	item := &Item{
		Path:        "/tmp/fallback.md",
		Frontmatter: map[string]interface{}{"title": "Found It"},
	}
	if got := item.Title(""); got != "Found It" {
		t.Errorf("Title('') = %q, want %q", got, "Found It")
	}
}

func TestItem_Title_CustomField(t *testing.T) {
	item := &Item{
		Path:        "/tmp/test.md",
		Frontmatter: map[string]interface{}{"heading": "Custom Heading"},
	}
	if got := item.Title("heading"); got != "Custom Heading" {
		t.Errorf("Title('heading') = %q, want %q", got, "Custom Heading")
	}
}

func TestItem_Subtitle_StringField(t *testing.T) {
	item := &Item{
		Frontmatter: map[string]interface{}{"author": "Jane Doe"},
	}
	if got := item.Subtitle("author"); got != "Jane Doe" {
		t.Errorf("Subtitle('author') = %q, want %q", got, "Jane Doe")
	}
}

func TestItem_Subtitle_ListField(t *testing.T) {
	item := &Item{
		Frontmatter: map[string]interface{}{
			"genres": []interface{}{"fiction", "sci-fi", "thriller"},
		},
	}
	if got := item.Subtitle("genres"); got != "fiction, sci-fi, thriller" {
		t.Errorf("Subtitle('genres') = %q, want %q", got, "fiction, sci-fi, thriller")
	}
}

func TestItem_Subtitle_EmptyField(t *testing.T) {
	item := &Item{
		Frontmatter: map[string]interface{}{"type": "note"},
	}
	if got := item.Subtitle(""); got != "" {
		t.Errorf("Subtitle('') = %q, want empty string", got)
	}
}

func TestItem_Subtitle_MissingField(t *testing.T) {
	item := &Item{
		Frontmatter: map[string]interface{}{"type": "note"},
	}
	if got := item.Subtitle("author"); got != "" {
		t.Errorf("Subtitle('author') = %q, want empty string", got)
	}
}

func TestItem_Tags(t *testing.T) {
	item := &Item{
		Frontmatter: map[string]interface{}{
			"tags": []interface{}{"go", "testing", "cli"},
		},
	}
	tags := item.GetTags()
	if len(tags) != 3 {
		t.Fatalf("GetTags() len = %d, want 3", len(tags))
	}
	expected := []string{"go", "testing", "cli"}
	for i, tag := range tags {
		if tag != expected[i] {
			t.Errorf("tags[%d] = %q, want %q", i, tag, expected[i])
		}
	}
}

func TestItem_Tags_Missing(t *testing.T) {
	item := &Item{
		Frontmatter: map[string]interface{}{"type": "note"},
	}
	tags := item.GetTags()
	if tags != nil {
		t.Errorf("GetTags() = %v, want nil", tags)
	}
}

func TestItem_Tags_Empty(t *testing.T) {
	item := &Item{
		Frontmatter: map[string]interface{}{
			"tags": []interface{}{},
		},
	}
	tags := item.GetTags()
	if len(tags) != 0 {
		t.Errorf("GetTags() len = %d, want 0", len(tags))
	}
}
