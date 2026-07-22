package exporter

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/markdown"
)

func TestExport(t *testing.T) {
	// 1. Setup temporary source directory
	srcDir := t.TempDir()
	
	// Item 1: Note
	note1Path := filepath.Join(srcDir, "note1.md")
	note1Content := `---
type: note
id: notes123
title: My First Note
created: "2026-06-30T12:00:00Z"
author: Erwin
tags: [test]
---
Note 1 body.
`
	if err := os.WriteFile(note1Path, []byte(note1Content), 0644); err != nil {
		t.Fatal(err)
	}

	// Item 2: Note with duplicate title
	note2Path := filepath.Join(srcDir, "note2.md")
	note2Content := `---
type: note
id: notes456
title: My First Note
created: "2026-06-30T15:00:00Z"
author: Erwin
tags: [duplicate]
---
Note 2 body.
`
	if err := os.WriteFile(note2Path, []byte(note2Content), 0644); err != nil {
		t.Fatal(err)
	}

	// Item 3: Book (different type and date)
	bookPath := filepath.Join(srcDir, "book.md")
	bookContent := `---
type: book
id: book789
title: My Book
created: "2025-05-15T09:00:00Z"
rating: 5
tags: [reading]
---
Book body.
`
	if err := os.WriteFile(bookPath, []byte(bookContent), 0644); err != nil {
		t.Fatal(err)
	}

	// Initialize cache
	c, err := cache.New(srcDir)
	if err != nil {
		t.Fatalf("failed to create cache: %v", err)
	}
	defer c.Close()

	// 2. Export all items
	destDir := t.TempDir()
	res := Export(c, ExportOptions{
		OutputDir: destDir,
	})

	if len(res.Errors) > 0 {
		t.Fatalf("export returned errors: %v", res.Errors)
	}
	if res.Exported != 3 {
		t.Errorf("expected 3 exported items, got %d", res.Exported)
	}

	// 3. Verify destination files exist
	expectedNote1 := filepath.Join(destDir, "note", "2026", "06", "my-first-note.md")
	expectedNote2 := filepath.Join(destDir, "note", "2026", "06", "my-first-note-1.md")
	expectedBook := filepath.Join(destDir, "book", "2025", "05", "my-book.md")

	if _, err := os.Stat(expectedNote1); err != nil {
		t.Errorf("expected note 1 to exist at %s, got: %v", expectedNote1, err)
	}
	if _, err := os.Stat(expectedNote2); err != nil {
		t.Errorf("expected note 2 to exist at %s, got: %v", expectedNote2, err)
	}
	if _, err := os.Stat(expectedBook); err != nil {
		t.Errorf("expected book to exist at %s, got: %v", expectedBook, err)
	}

	// 4. Verify frontmatter filter removes type, id, and created
	data, err := os.ReadFile(expectedNote1)
	if err != nil {
		t.Fatal(err)
	}
	fm, body, err := markdown.ParseFrontmatterBytes(data)
	if err != nil {
		t.Fatal(err)
	}

	if _, exists := fm["type"]; exists {
		t.Error("expected 'type' field to be removed")
	}
	if _, exists := fm["id"]; exists {
		t.Error("expected 'id' field to be removed")
	}
	if _, exists := fm["created"]; exists {
		t.Error("expected 'created' field to be removed")
	}
	if fm["author"] != "Erwin" {
		t.Errorf("expected 'author' field to be preserved, got %v", fm["author"])
	}
	if !strings.Contains(body, "Note 1 body.") {
		t.Errorf("expected body to be preserved, got %q", body)
	}

	// 5. Test type filtering: export books only
	destDirBook := t.TempDir()
	resBook := Export(c, ExportOptions{
		OutputDir:  destDirBook,
		TargetType: "book",
	})

	if len(resBook.Errors) > 0 {
		t.Fatalf("export returned errors: %v", resBook.Errors)
	}
	if resBook.Exported != 1 {
		t.Errorf("expected 1 exported item, got %d", resBook.Exported)
	}

	expectedBookOnly := filepath.Join(destDirBook, "book", "2025", "05", "my-book.md")
	if _, err := os.Stat(expectedBookOnly); err != nil {
		t.Errorf("expected book to exist at %s, got: %v", expectedBookOnly, err)
	}
	
	// Ensure note was not exported
	unexpectedNote := filepath.Join(destDirBook, "note", "2026", "06", "my-first-note.md")
	if _, err := os.Stat(unexpectedNote); !os.IsNotExist(err) {
		t.Errorf("expected note NOT to be exported, but it exists at %s", unexpectedNote)
	}
}
