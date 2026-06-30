package cache

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func writeTestFile(t *testing.T, path, content string) {
	t.Helper()
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		t.Fatalf("mkdir %s: %v", dir, err)
	}
	if err := os.WriteFile(path, []byte(content), 0644); err != nil {
		t.Fatalf("write %s: %v", path, err)
	}
}

func TestNew_StartsSuccessfully(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "note.md"), "---\ntitle: Test\ntype: note\n---\nBody\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()
}

func TestAll_ReturnsAllMDFiles(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "notes", "one.md"), "---\ntitle: One\ntype: note\n---\nBody one\n")
	writeTestFile(t, filepath.Join(dir, "notes", "two.md"), "---\ntitle: Two\ntype: note\n---\nBody two\n")
	writeTestFile(t, filepath.Join(dir, "books", "book.md"), "---\ntitle: A Book\ntype: book\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if len(items) != 3 {
		t.Fatalf("expected 3 items, got %d", len(items))
	}
}

func TestAll_SkipsFilesWithoutType(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "typed.md"), "---\ntitle: Typed\ntype: note\n---\n")
	writeTestFile(t, filepath.Join(dir, "untyped.md"), "---\ntitle: No Type\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 item (untyped skipped), got %d", len(items))
	}
	if items[0].Type != "note" {
		t.Errorf("Type = %q, want 'note'", items[0].Type)
	}
}

func TestAll_SkipsHiddenDirs(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "visible.md"), "---\ntitle: Vis\ntype: note\n---\n")
	writeTestFile(t, filepath.Join(dir, ".git", "config.md"), "---\ntitle: Git\ntype: note\n---\n")
	writeTestFile(t, filepath.Join(dir, ".exo", "data.md"), "---\ntitle: Exo\ntype: note\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 item (hidden dirs skipped), got %d", len(items))
	}
}

func TestAll_SkipsNonMDFiles(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "note.md"), "---\ntitle: Note\ntype: note\n---\n")
	writeTestFile(t, filepath.Join(dir, "readme.txt"), "plain text")
	writeTestFile(t, filepath.Join(dir, "data.json"), "{}")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 item (.md only), got %d", len(items))
	}
}

func TestGetByPrefix(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "notes", "one.md"), "---\ntitle: One\ntype: note\n---\n")
	writeTestFile(t, filepath.Join(dir, "notes", "two.md"), "---\ntitle: Two\ntype: note\n---\n")
	writeTestFile(t, filepath.Join(dir, "books", "book.md"), "---\ntitle: Book\ntype: book\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	items, err := c.GetByPrefix("notes/")
	if err != nil {
		t.Fatalf("GetByPrefix: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 notes, got %d", len(items))
	}

	items, err = c.GetByPrefix("books/")
	if err != nil {
		t.Fatalf("GetByPrefix: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 book, got %d", len(items))
	}
}

func TestGet_SingleItem(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "notes", "test.md"), "---\ntitle: Test Note\ntype: note\ntags:\n  - go\n---\nContent\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	item, err := c.Get("notes/test.md")
	if err != nil {
		t.Fatalf("Get: %v", err)
	}
	if item.Frontmatter["title"] != "Test Note" {
		t.Errorf("title = %v, want 'Test Note'", item.Frontmatter["title"])
	}
	if item.Type != "note" {
		t.Errorf("Type = %q, want 'note'", item.Type)
	}
}

func TestGet_NotFound(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "note.md"), "---\ntitle: X\ntype: note\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	_, err = c.Get("nonexistent.md")
	if err == nil {
		t.Error("expected error for non-existent key")
	}
}

func TestGetByType(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "note1.md"), "---\ntitle: Note 1\ntype: note\n---\n")
	writeTestFile(t, filepath.Join(dir, "note2.md"), "---\ntitle: Note 2\ntype: note\n---\n")
	writeTestFile(t, filepath.Join(dir, "book.md"), "---\ntitle: Book 1\ntype: book\n---\n")
	writeTestFile(t, filepath.Join(dir, "article.md"), "---\ntitle: Article 1\ntype: article\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	notes, err := c.GetByType("note")
	if err != nil {
		t.Fatalf("GetByType(note): %v", err)
	}
	if len(notes) != 2 {
		t.Fatalf("expected 2 notes, got %d", len(notes))
	}

	items, err := c.GetByType("book", "article")
	if err != nil {
		t.Fatalf("GetByType(book, article): %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 items (book+article), got %d", len(items))
	}

	items, err = c.GetByType("nonexistent")
	if err != nil {
		t.Fatalf("GetByType(nonexistent): %v", err)
	}
	if len(items) != 0 {
		t.Fatalf("expected 0 items for non-existent type, got %d", len(items))
	}
}

func TestGetByTag(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "a.md"), "---\ntitle: A\ntype: note\ntags:\n  - go\n  - testing\n---\n")
	writeTestFile(t, filepath.Join(dir, "b.md"), "---\ntitle: B\ntype: note\ntags:\n  - go\n  - rust\n---\n")
	writeTestFile(t, filepath.Join(dir, "c.md"), "---\ntitle: C\ntype: note\ntags:\n  - python\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	goItems, err := c.GetByTag("go")
	if err != nil {
		t.Fatalf("GetByTag(go): %v", err)
	}
	if len(goItems) != 2 {
		t.Fatalf("expected 2 items with 'go' tag, got %d", len(goItems))
	}

	pyItems, err := c.GetByTag("python")
	if err != nil {
		t.Fatalf("GetByTag(python): %v", err)
	}
	if len(pyItems) != 1 {
		t.Fatalf("expected 1 item with 'python' tag, got %d", len(pyItems))
	}

	noneItems, err := c.GetByTag("nonexistent")
	if err != nil {
		t.Fatalf("GetByTag(nonexistent): %v", err)
	}
	if len(noneItems) != 0 {
		t.Fatalf("expected 0 items for non-existent tag, got %d", len(noneItems))
	}
}

func TestAutoPopulate_ID(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	writeTestFile(t, path, "---\ntitle: No ID\ntype: note\ntags: []\n---\nBody\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 item, got %d", len(items))
	}

	if items[0].ID == "" {
		t.Error("ID should have been auto-populated")
	}

	// Verify the file was rewritten with the ID
	content, _ := os.ReadFile(path)
	if !strings.Contains(string(content), "id:") {
		t.Error("file should have been rewritten with id field")
	}
}

func TestAutoPopulate_Created(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	writeTestFile(t, path, "---\ntitle: No Date\ntype: note\ntags: []\nid: abc\n---\nBody\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if items[0].Created.IsZero() {
		t.Error("Created should have been auto-populated from file mtime")
	}

	content, _ := os.ReadFile(path)
	if !strings.Contains(string(content), "created:") {
		t.Error("file should have been rewritten with created field")
	}
}

func TestAutoPopulate_Tags(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	writeTestFile(t, path, "---\ntitle: No Tags\ntype: note\nid: abc\ncreated: 2024-01-01\n---\nBody\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if items[0].Tags == nil {
		t.Error("Tags should have been auto-populated as empty slice")
	}

	content, _ := os.ReadFile(path)
	if !strings.Contains(string(content), "tags:") {
		t.Error("file should have been rewritten with tags field")
	}
}

func TestWatcher_DetectsNewFile(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "initial.md"), "---\ntitle: Initial\ntype: note\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	writeTestFile(t, filepath.Join(dir, "new.md"), "---\ntitle: New\ntype: note\n---\nNew body\n")

	time.Sleep(200 * time.Millisecond)

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 items after adding file, got %d", len(items))
	}
}

func TestWatcher_DetectsModifiedFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	writeTestFile(t, path, "---\ntitle: Original\ntype: note\ntags: []\nid: x\ncreated: 2024-01-01\n---\nOriginal body\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	os.WriteFile(path, []byte("---\ntitle: Updated\ntype: note\ntags: []\nid: x\ncreated: 2024-01-01\n---\nUpdated body\n"), 0644)

	time.Sleep(200 * time.Millisecond)

	item, err := c.Get("note.md")
	if err != nil {
		t.Fatalf("Get: %v", err)
	}
	if item.Frontmatter["title"] != "Updated" {
		t.Errorf("title = %v, want 'Updated'", item.Frontmatter["title"])
	}
}

func TestWatcher_DetectsDeletedFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	writeTestFile(t, path, "---\ntitle: ToDelete\ntype: note\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	os.Remove(path)

	time.Sleep(200 * time.Millisecond)

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if len(items) != 0 {
		t.Fatalf("expected 0 items after delete, got %d", len(items))
	}
}

func TestWatcher_DetectsNewDirectory(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "root.md"), "---\ntitle: Root\ntype: note\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	subdir := filepath.Join(dir, "newdir")
	os.MkdirAll(subdir, 0755)
	time.Sleep(100 * time.Millisecond)

	writeTestFile(t, filepath.Join(subdir, "sub.md"), "---\ntitle: Sub\ntype: note\n---\n")

	time.Sleep(200 * time.Millisecond)

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 items after new dir + file, got %d", len(items))
	}
}

func TestSync_RemovesStaleEntries(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	writeTestFile(t, path, "---\ntitle: Temp\ntype: note\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	os.Remove(path)

	if err := c.Sync(); err != nil {
		t.Fatalf("Sync: %v", err)
	}

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if len(items) != 0 {
		t.Fatalf("expected 0 items after sync, got %d", len(items))
	}
}

func TestNotifyWrite(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	writeTestFile(t, path, "---\ntitle: V1\ntype: note\ntags: []\nid: x\ncreated: 2024-01-01\n---\nV1 body\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	// Update file and notify
	os.WriteFile(path, []byte("---\ntitle: V2\ntype: note\ntags: [updated]\nid: x\ncreated: 2024-01-01\n---\nV2 body\n"), 0644)
	if err := c.NotifyWrite(path); err != nil {
		t.Fatalf("NotifyWrite: %v", err)
	}

	item, err := c.Get("note.md")
	if err != nil {
		t.Fatalf("Get: %v", err)
	}
	if item.Frontmatter["title"] != "V2" {
		t.Errorf("title = %v, want 'V2'", item.Frontmatter["title"])
	}
	if item.Body != "V2 body\n" {
		t.Errorf("body = %q, want 'V2 body\\n'", item.Body)
	}
}

func TestNotifyDelete(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	writeTestFile(t, path, "---\ntitle: X\ntype: note\ntags: [go]\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	os.Remove(path)
	if err := c.NotifyDelete(path); err != nil {
		t.Fatalf("NotifyDelete: %v", err)
	}

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if len(items) != 0 {
		t.Fatalf("expected 0 items after NotifyDelete, got %d", len(items))
	}

	// Indexes should be cleaned up too
	goItems, _ := c.GetByTag("go")
	if len(goItems) != 0 {
		t.Errorf("expected 0 go-tagged items after delete, got %d", len(goItems))
	}
	noteItems, _ := c.GetByType("note")
	if len(noteItems) != 0 {
		t.Errorf("expected 0 note-typed items after delete, got %d", len(noteItems))
	}
}

func TestIndexUpdatedOnModify(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	writeTestFile(t, path, "---\ntitle: Test\ntype: note\ntags:\n  - go\nid: x\ncreated: 2024-01-01\n---\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	// Update to different type and tags
	os.WriteFile(path, []byte("---\ntitle: Test\ntype: book\ntags:\n  - rust\nid: x\ncreated: 2024-01-01\n---\n"), 0644)
	c.NotifyWrite(path)

	// Old indexes should be empty
	notes, _ := c.GetByType("note")
	if len(notes) != 0 {
		t.Errorf("expected 0 notes after type change, got %d", len(notes))
	}
	goItems, _ := c.GetByTag("go")
	if len(goItems) != 0 {
		t.Errorf("expected 0 go-tagged items after tag change, got %d", len(goItems))
	}

	// New indexes should have the entry
	books, _ := c.GetByType("book")
	if len(books) != 1 {
		t.Errorf("expected 1 book after type change, got %d", len(books))
	}
	rustItems, _ := c.GetByTag("rust")
	if len(rustItems) != 1 {
		t.Errorf("expected 1 rust-tagged item after tag change, got %d", len(rustItems))
	}
}

func TestTypedFields(t *testing.T) {
	dir := t.TempDir()
	writeTestFile(t, filepath.Join(dir, "test.md"), "---\ntitle: My Note\ntags:\n  - go\n  - testing\ntype: note\nid: abc123\ncreated: 2024-06-15\n---\nBody content\n")

	c, err := New(dir)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer c.Close()

	items, err := c.All()
	if err != nil {
		t.Fatalf("All: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 item, got %d", len(items))
	}

	item := items[0]
	if item.Type != "note" {
		t.Errorf("Type = %q, want 'note'", item.Type)
	}
	if item.ID != "abc123" {
		t.Errorf("ID = %q, want 'abc123'", item.ID)
	}
	if len(item.Tags) != 2 || item.Tags[0] != "go" || item.Tags[1] != "testing" {
		t.Errorf("Tags = %v, want [go testing]", item.Tags)
	}
	if item.Created.Year() != 2024 || item.Created.Month() != 6 || item.Created.Day() != 15 {
		t.Errorf("Created = %v, want 2024-06-15", item.Created)
	}
	if item.Body != "Body content\n" {
		t.Errorf("Body = %q, want 'Body content\\n'", item.Body)
	}
}

func TestScan_MigratesLegacyItems(t *testing.T) {
	dir := t.TempDir()

	// Write a file with a legacy 9-character base62 ID under an old directory structure
	oldSubdir := filepath.Join(dir, "zettelkasten")
	if err := os.MkdirAll(oldSubdir, 0755); err != nil {
		t.Fatal(err)
	}
	oldPath := filepath.Join(oldSubdir, "1IRw6kygS-legacy.md")
	content := `---
type: note
id: 1IRw6kygS
title: Legacy Note
created: "2020-01-15T12:00:00Z"
---
This is a legacy body.
`
	if err := os.WriteFile(oldPath, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	// Initialize the Cache (this will run scan, which should trigger the migration)
	c, err := New(dir)
	if err != nil {
		t.Fatalf("Failed to create cache: %v", err)
	}
	defer c.Close()

	// 1. Verify old file is deleted
	if _, err := os.Stat(oldPath); !os.IsNotExist(err) {
		t.Errorf("expected old file to be deleted, but it still exists")
	}

	// 2. Verify new file exists in the correct prefix directory
	items, err := c.All()
	if err != nil {
		t.Fatal(err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 migrated item, got %d", len(items))
	}

	migrated := items[0]
	if len(migrated.ID) != 7 {
		t.Errorf("expected migrated ID to be 7 characters, got %q", migrated.ID)
	}

	// Filename should be <newID>-legacy-note.md
	expectedFilename := migrated.ID + "-legacy-note.md"
	expectedDir := filepath.Join(dir, migrated.ID[:3])
	expectedPath := filepath.Join(expectedDir, expectedFilename)

	if _, err := os.Stat(expectedPath); err != nil {
		t.Errorf("expected migrated file to exist at %s, but got error: %v", expectedPath, err)
	}

	// 3. Verify content was rewritten with new ID
	data, err := os.ReadFile(expectedPath)
	if err != nil {
		t.Fatal(err)
	}
	fileContent := string(data)
	if !strings.Contains(fileContent, "id: "+migrated.ID) {
		t.Errorf("expected file frontmatter to be updated with new ID %s, got:\n%s", migrated.ID, fileContent)
	}
	if !strings.Contains(fileContent, "This is a legacy body.") {
		t.Errorf("lost body content: %s", fileContent)
	}
}


