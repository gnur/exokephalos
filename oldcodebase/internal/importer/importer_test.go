package importer

import (
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

func TestImport_BasicFile(t *testing.T) {
	// Create temp directories
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	// Create a simple markdown file
	sourceFile := filepath.Join(sourceDir, "test.md")
	content := `---
title: Test Note
tags: [foo, bar]
---

# Test Note

This is the body.
`
	if err := os.WriteFile(sourceFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	// Import
	result := Import(sourceDir, exoDir, "note")

	// Verify
	if result.Imported != 1 {
		t.Errorf("expected 1 imported, got %d", result.Imported)
	}
	if result.Skipped != 0 {
		t.Errorf("expected 0 skipped, got %d", result.Skipped)
	}
	if len(result.Errors) != 0 {
		t.Errorf("expected 0 errors, got %d: %v", len(result.Errors), result.Errors)
	}

	// Find the imported file
	var importedFile string
	filepath.Walk(exoDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if !info.IsDir() && filepath.Ext(path) == ".md" {
			importedFile = path
		}
		return nil
	})

	if importedFile == "" {
		t.Fatal("no imported file found")
	}

	// Read and verify the imported file
	data, err := os.ReadFile(importedFile)
	if err != nil {
		t.Fatal(err)
	}

	content = string(data)
	if !strings.Contains(content, "type: note") {
		t.Error("expected type: note in frontmatter")
	}
	if !strings.Contains(content, "title: Test Note") {
		t.Error("expected title: Test Note in frontmatter")
	}
	if !strings.Contains(content, "tags:") {
		t.Error("expected tags in frontmatter")
	}
	if !strings.Contains(content, "# Test Note") {
		t.Error("expected body to contain # Test Note")
	}
}

func TestImport_PreservesExistingType(t *testing.T) {
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	sourceFile := filepath.Join(sourceDir, "test.md")
	content := `---
type: article
title: My Article
---

Content here.
`
	if err := os.WriteFile(sourceFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	result := Import(sourceDir, exoDir, "note")

	if result.Imported != 1 {
		t.Errorf("expected 1 imported, got %d", result.Imported)
	}

	// Find and read the imported file
	var importedFile string
	filepath.Walk(exoDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if !info.IsDir() && filepath.Ext(path) == ".md" {
			importedFile = path
		}
		return nil
	})

	data, err := os.ReadFile(importedFile)
	if err != nil {
		t.Fatal(err)
	}

	if !strings.Contains(string(data), "type: article") {
		t.Error("expected type: article to be preserved, not overridden to note")
	}
}

func TestImport_GeneratesIDFromCreated(t *testing.T) {
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	sourceFile := filepath.Join(sourceDir, "test.md")
	content := `---
title: Old Note
created: 2020-01-15
---

Body.
`
	if err := os.WriteFile(sourceFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	result := Import(sourceDir, exoDir, "note")

	if result.Imported != 1 {
		t.Errorf("expected 1 imported, got %d", result.Imported)
	}

	// Find the imported file
	var importedFile string
	filepath.Walk(exoDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if !info.IsDir() && filepath.Ext(path) == ".md" {
			importedFile = path
		}
		return nil
	})

	// The filename should be the ID followed by the title
	filename := filepath.Base(importedFile)
	base := strings.TrimSuffix(filename, ".md")
	var idStr string
	if idx := strings.Index(base, "-"); idx != -1 {
		idStr = base[:idx]
	} else {
		idStr = base
	}

	// ID should be 7 characters
	if len(idStr) != 7 {
		t.Errorf("expected ID length 7, got %d: %s (filename: %s)", len(idStr), idStr, filename)
	}

	// The directory should be first 3 chars of ID
	dir := filepath.Base(filepath.Dir(importedFile))
	if dir != idStr[:3] {
		t.Errorf("expected directory %s, got %s", idStr[:3], dir)
	}
}

func TestImport_SkipsExistingFiles(t *testing.T) {
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	sourceFile := filepath.Join(sourceDir, "test.md")
	content := `---
title: Test
---

Body.
`
	if err := os.WriteFile(sourceFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	// Import once
	result1 := Import(sourceDir, exoDir, "note")
	if result1.Imported != 1 {
		t.Errorf("first import: expected 1 imported, got %d", result1.Imported)
	}

	// Import again
	result2 := Import(sourceDir, exoDir, "note")
	if result2.Imported != 0 {
		t.Errorf("second import: expected 0 imported, got %d", result2.Imported)
	}
	if result2.Skipped != 1 {
		t.Errorf("second import: expected 1 skipped, got %d", result2.Skipped)
	}
}

func TestImport_Recursive(t *testing.T) {
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	// Create nested structure
	subdir := filepath.Join(sourceDir, "subdir", "nested")
	if err := os.MkdirAll(subdir, 0755); err != nil {
		t.Fatal(err)
	}

	// Create files at different levels
	files := []string{
		filepath.Join(sourceDir, "root.md"),
		filepath.Join(sourceDir, "subdir", "level1.md"),
		filepath.Join(subdir, "level2.md"),
	}

	for _, f := range files {
		content := "---\ntitle: Test\n---\nBody"
		if err := os.WriteFile(f, []byte(content), 0644); err != nil {
			t.Fatal(err)
		}
	}

	result := Import(sourceDir, exoDir, "note")

	if result.Imported != 3 {
		t.Errorf("expected 3 imported, got %d", result.Imported)
	}
}

func TestImport_ExtractsTitleFromHeader(t *testing.T) {
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	sourceFile := filepath.Join(sourceDir, "test.md")
	content := `# My Header Title

This is the body.
`
	if err := os.WriteFile(sourceFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	result := Import(sourceDir, exoDir, "note")

	if result.Imported != 1 {
		t.Errorf("expected 1 imported, got %d", result.Imported)
	}

	// Find and read the imported file
	var importedFile string
	filepath.Walk(exoDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if !info.IsDir() && filepath.Ext(path) == ".md" {
			importedFile = path
		}
		return nil
	})

	data, err := os.ReadFile(importedFile)
	if err != nil {
		t.Fatal(err)
	}

	if !strings.Contains(string(data), "title: My Header Title") {
		t.Error("expected title to be extracted from # Header")
	}
}

func TestImport_FallsBackToFilename(t *testing.T) {
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	sourceFile := filepath.Join(sourceDir, "my-cool-file.md")
	content := `Just some text without frontmatter or headers.`
	if err := os.WriteFile(sourceFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	result := Import(sourceDir, exoDir, "note")

	if result.Imported != 1 {
		t.Errorf("expected 1 imported, got %d", result.Imported)
	}

	// Find and read the imported file
	var importedFile string
	filepath.Walk(exoDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if !info.IsDir() && filepath.Ext(path) == ".md" {
			importedFile = path
		}
		return nil
	})

	data, err := os.ReadFile(importedFile)
	if err != nil {
		t.Fatal(err)
	}

	if !strings.Contains(string(data), "title: my-cool-file") {
		t.Error("expected title to fall back to filename")
	}
}

func TestImport_UsesFileModTime(t *testing.T) {
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	sourceFile := filepath.Join(sourceDir, "test.md")
	content := `# No Frontmatter

Just body.
`
	if err := os.WriteFile(sourceFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	// Set a specific mod time
	modTime := time.Date(2023, 6, 15, 12, 0, 0, 0, time.UTC)
	if err := os.Chtimes(sourceFile, modTime, modTime); err != nil {
		t.Fatal(err)
	}

	result := Import(sourceDir, exoDir, "note")

	if result.Imported != 1 {
		t.Errorf("expected 1 imported, got %d", result.Imported)
	}

	// Find and read the imported file
	var importedFile string
	filepath.Walk(exoDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if !info.IsDir() && filepath.Ext(path) == ".md" {
			importedFile = path
		}
		return nil
	})

	data, err := os.ReadFile(importedFile)
	if err != nil {
		t.Fatal(err)
	}

	importedContent := string(data)
	t.Logf("Imported file content:\n%s", importedContent)

	// YAML encoder may quote the date, so check for both formats
	if !strings.Contains(importedContent, "created: 2023-06-15") && !strings.Contains(importedContent, `created: "2023-06-15"`) {
		t.Errorf("expected created to use file mod time, got:\n%s", importedContent)
	}
}

func TestImport_UnquotedDate(t *testing.T) {
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	sourceFile := filepath.Join(sourceDir, "test.md")
	// Note: unquoted date in YAML parses as time.Time
	content := `---
title: Unquoted Date
created: 2026-06-08
---

Body.
`
	if err := os.WriteFile(sourceFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	result := Import(sourceDir, exoDir, "note")

	if result.Imported != 1 {
		t.Errorf("expected 1 imported, got %d", result.Imported)
	}
	if len(result.Errors) > 0 {
		t.Errorf("unexpected errors: %v", result.Errors)
	}

	// Find the imported file
	var importedFile string
	filepath.Walk(exoDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if !info.IsDir() && filepath.Ext(path) == ".md" {
			importedFile = path
		}
		return nil
	})

	data, err := os.ReadFile(importedFile)
	if err != nil {
		t.Fatal(err)
	}

	importedContent := string(data)
	if !strings.Contains(importedContent, "created: 2026-06-08") && !strings.Contains(importedContent, `created: "2026-06-08"`) {
		t.Errorf("expected created to be 2026-06-08, got:\n%s", importedContent)
	}
}

func TestImport_NoCollisions(t *testing.T) {
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	// Create 40 files with the same created date but different source paths
	for i := 0; i < 40; i++ {
		sourceFile := filepath.Join(sourceDir, filepath.Join(t.Name(), string(rune(65+i%26))+strconv.Itoa(i)+".md"))
		if err := os.MkdirAll(filepath.Dir(sourceFile), 0755); err != nil {
			t.Fatal(err)
		}
		content := `---
title: Book ` + strconv.Itoa(i) + `
created: "2026-06-08"
---

Body.
`
		if err := os.WriteFile(sourceFile, []byte(content), 0644); err != nil {
			t.Fatal(err)
		}
	}

	result := Import(sourceDir, exoDir, "book")

	if result.Imported != 40 {
		t.Errorf("expected 40 imported, got %d. Errors: %v", result.Imported, result.Errors)
	}
	if result.Skipped != 0 {
		t.Errorf("expected 0 skipped, got %d", result.Skipped)
	}
}

func TestImport_CustomID(t *testing.T) {
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	// Test case 1: 5-character string ID (like d3f4d, invalid format) -> should be updated
	sourceFile1 := filepath.Join(sourceDir, "book1.md")
	content1 := `---
title: A Parade of Horribles
id: d3f4d
created: "2026-06-08"
---
Body.
`
	if err := os.WriteFile(sourceFile1, []byte(content1), 0644); err != nil {
		t.Fatal(err)
	}

	// Test case 2: Unquoted integer ID (like 12345, invalid format) -> should be updated
	sourceFile2 := filepath.Join(sourceDir, "book2.md")
	content2 := `---
title: Another Book
id: 12345
created: "2026-06-08"
---
Body.
`
	if err := os.WriteFile(sourceFile2, []byte(content2), 0644); err != nil {
		t.Fatal(err)
	}

	// Test case 3: Valid 9-char ID format (matching 2020-01-15 timestamp) -> should be preserved
	sourceFile3 := filepath.Join(sourceDir, "book3.md")
	content3 := `---
title: Valid ID Note
id: 1IRw6kygS
created: "2020-01-15"
---
Body.
`
	if err := os.WriteFile(sourceFile3, []byte(content3), 0644); err != nil {
		t.Fatal(err)
	}

	result := Import(sourceDir, exoDir, "book")
	if result.Imported != 3 {
		t.Errorf("expected 3 imported, got %d. Errors: %v", result.Imported, result.Errors)
	}

	// Verify book3 has its valid ID preserved: 1IRw6kygS
	destFile3 := filepath.Join(exoDir, "1IR", "1IRw6kygS-valid-id-note.md")
	if _, err := os.Stat(destFile3); err != nil {
		t.Errorf("expected book3 to preserve valid ID at %s, got err: %v", destFile3, err)
	}

	// Find the other two files to verify they got updated IDs (9 chars) and filenames include title
	var files []string
	filepath.Walk(exoDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if !info.IsDir() && filepath.Ext(path) == ".md" {
			files = append(files, path)
		}
		return nil
	})

	if len(files) != 3 {
		t.Fatalf("expected 3 files on disk, got %d: %v", len(files), files)
	}

	foundHorribles := false
	foundAnother := false

	for _, f := range files {
		filename := filepath.Base(f)
		data, err := os.ReadFile(f)
		if err != nil {
			t.Fatal(err)
		}
		content := string(data)

		if strings.Contains(filename, "a-parade-of-horribles") {
			foundHorribles = true
			// ID in filename prefix should be 7 characters
			idPart := filename[:7]
			if !strings.Contains(content, "id: " + idPart) {
				t.Errorf("expected book1 frontmatter ID to be updated to %s, got:\n%s", idPart, content)
			}
		} else if strings.Contains(filename, "another-book") {
			foundAnother = true
			idPart := filename[:7]
			if !strings.Contains(content, "id: " + idPart) {
				t.Errorf("expected book2 frontmatter ID to be updated to %s, got:\n%s", idPart, content)
			}
		}
	}

	if !foundHorribles {
		t.Error("expected to find imported file for 'A Parade of Horribles'")
	}
	if !foundAnother {
		t.Error("expected to find imported file for 'Another Book'")
	}
}

func TestImport_FormatsAndPreservesAllTimestamps(t *testing.T) {
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	sourceFile := filepath.Join(sourceDir, "book.md")
	content := `---
added: "2026-06-05"
author:
  - Matt Dinniman
created: "2026-06-08"
finished: 2026-06-22
id: d3f4d
pages: 704
tags:
  - read
title: 'A Parade of Horribles (Dungeon Crawler Carl, #8)'
type: book
---
Body.
`
	if err := os.WriteFile(sourceFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	result := Import(sourceDir, exoDir, "book")
	if result.Imported != 1 {
		t.Fatalf("expected 1 imported, got %d. Errors: %v", result.Imported, result.Errors)
	}

	// Find the imported file dynamically
	var destFile string
	filepath.Walk(exoDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if !info.IsDir() && filepath.Ext(path) == ".md" {
			destFile = path
		}
		return nil
	})

	if destFile == "" {
		t.Fatal("no imported file found")
	}

	data, err := os.ReadFile(destFile)
	if err != nil {
		t.Fatal(err)
	}

	importedContent := string(data)
	t.Logf("Imported file content:\n%s", importedContent)

	// Ensure all timestamps are unquoted
	if !strings.Contains(importedContent, "added: 2026-06-05T00:00:00Z\n") {
		t.Errorf("expected 'added: 2026-06-05T00:00:00Z' without quotes, got:\n%s", importedContent)
	}
	if !strings.Contains(importedContent, "created: 2026-06-08T00:00:00Z\n") {
		t.Errorf("expected 'created: 2026-06-08T00:00:00Z' without quotes, got:\n%s", importedContent)
	}
	if !strings.Contains(importedContent, "finished: 2026-06-22T00:00:00Z\n") {
		t.Errorf("expected 'finished: 2026-06-22T00:00:00Z' without quotes, got:\n%s", importedContent)
	}

	// Ensure other fields are preserved
	if !strings.Contains(importedContent, "author:\n  - Matt Dinniman") {
		t.Errorf("expected author Matt Dinniman to be preserved, got:\n%s", importedContent)
	}
	if !strings.Contains(importedContent, "pages: 704") {
		t.Errorf("expected pages: 704 to be preserved, got:\n%s", importedContent)
	}
}


