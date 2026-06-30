package tui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/filter"
	"github.com/gnur/exokephalos/internal/importer"
	"github.com/gnur/exokephalos/internal/markdown"
)

func setupTestRepo(t *testing.T) string {
	tmpDir := t.TempDir()

	// Copy the .exo configuration directory
	srcConfigDir := filepath.Join("../../example-repo", ".exo")
	destConfigDir := filepath.Join(tmpDir, ".exo")
	if err := os.MkdirAll(destConfigDir, 0755); err != nil {
		t.Fatal(err)
	}

	// Copy config files
	files, err := os.ReadDir(srcConfigDir)
	if err != nil {
		t.Fatal(err)
	}
	for _, f := range files {
		if f.IsDir() && f.Name() == "cache" {
			continue // skip cache
		}
		data, err := os.ReadFile(filepath.Join(srcConfigDir, f.Name()))
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(destConfigDir, f.Name()), data, 0644); err != nil {
			t.Fatal(err)
		}
	}

	// Import each type from the raw folders in example-repo
	importer.Import("../../example-repo/book", tmpDir, "book")
	importer.Import("../../example-repo/note", tmpDir, "note")
	importer.Import("../../example-repo/webhook", tmpDir, "webhook")
	importer.Import("../../example-repo/secret", tmpDir, "secret")

	return tmpDir
}

func TestViewFilterIntegration(t *testing.T) {
	tmpDir := setupTestRepo(t)

	// Load the example config
	cfg, err := config.Load(tmpDir)
	if err != nil {
		t.Fatalf("Failed to load config: %v", err)
	}

	// Scan using cache
	c, err := cache.New(tmpDir)
	if err != nil {
		t.Fatalf("Failed to create cache: %v", err)
	}
	defer c.Close()

	items, err := c.All()
	if err != nil {
		t.Fatalf("Failed to get items from cache: %v", err)
	}

	if len(items) == 0 {
		t.Fatal("No items scanned from example-repo")
	}

	t.Logf("Scanned %d items total", len(items))

	// Test each view's filter matches some items
	for id, viewCfg := range cfg.Views {
		prog, err := filter.Compile(viewCfg.Filter)
		if err != nil {
			t.Errorf("View %q: filter compile error: %v", id, err)
			continue
		}

		var matched int
		for _, item := range items {
			ok, err := prog.Eval(item.Frontmatter)
			if err != nil {
				t.Errorf("View %q: eval error on %s: %v", id, item.Path, err)
				continue
			}
			if ok {
				matched++
			}
		}

		t.Logf("View %q (%s): %d items matched", id, viewCfg.Name, matched)

		if matched == 0 {
			t.Errorf("View %q: no items matched filter %q", id, viewCfg.Filter)
		}

		// Test subview filters narrow correctly
		for i, sv := range viewCfg.Subviews {
			subProg, err := filter.Compile(sv.Filter)
			if err != nil {
				t.Errorf("View %q subview %q: compile error: %v", id, sv.Name, err)
				continue
			}

			var subMatched int
			for _, item := range items {
				parentOk, _ := prog.Eval(item.Frontmatter)
				if !parentOk {
					continue
				}
				subOk, _ := subProg.Eval(item.Frontmatter)
				if subOk {
					subMatched++
				}
			}

			t.Logf("  Subview %d %q: %d items", i, sv.Name, subMatched)

			// "All" subview should match everything from parent
			if sv.Filter == "true" && subMatched != matched {
				t.Errorf("View %q subview %q with filter 'true': got %d items, expected %d", id, sv.Name, subMatched, matched)
			}
		}
	}
}

func TestEnsureIDInFrontmatter_AlreadyPresent(t *testing.T) {
	content := "---\nid: abc12\ntype: note\ntitle: Test\n---\n\nBody"
	result := markdown.EnsureID(content, "xyz99")
	if result != content {
		t.Errorf("should not modify content when id exists.\nGot:\n%s", result)
	}
}

func TestEnsureIDInFrontmatter_Missing(t *testing.T) {
	content := "---\ntype: article\ntimestamp: 2025-01-01\n---\n\nBody"
	result := markdown.EnsureID(content, "abc12")
	if !strings.Contains(result, "id: abc12") {
		t.Errorf("expected id to be injected.\nGot:\n%s", result)
	}
	// Should still have the other fields
	if !strings.Contains(result, "type: article") {
		t.Errorf("lost type field.\nGot:\n%s", result)
	}
	if !strings.Contains(result, "Body") {
		t.Errorf("lost body.\nGot:\n%s", result)
	}
}

func TestEnsureIDInFrontmatter_NoFrontmatter(t *testing.T) {
	content := "Just some text without frontmatter"
	result := markdown.EnsureID(content, "abc12")
	if !strings.Contains(result, "id: abc12") {
		t.Errorf("expected id to be injected.\nGot:\n%s", result)
	}
	if !strings.Contains(result, "Just some text") {
		t.Errorf("lost original content.\nGot:\n%s", result)
	}
}

func TestRenderCreateTemplate_InjectsID(t *testing.T) {
	// Template without id field
	tmpl := "---\ntype: article\ntimestamp: {{.DateTime}}\n---\n"

	vars := newAutoFillVars()
	content, _, err := renderCreateTemplate(tmpl, "articles", "/tmp", vars)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(content, "id: ") {
		t.Errorf("expected id to be auto-injected.\nGot:\n%s", content)
	}
}

func TestRenderCreateTemplate_KeepsExistingID(t *testing.T) {
	// Template with explicit id field
	tmpl := "---\ntype: note\nid: {{.ID}}\ntitle: {{.Title}}\n---\n"

	vars := newAutoFillVars()
	vars["Title"] = "Test Note"
	content, _, err := renderCreateTemplate(tmpl, "notes", "/tmp", vars)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	// Should have exactly one id field (not duplicated)
	count := strings.Count(content, "id: ")
	if count != 1 {
		t.Errorf("expected exactly 1 id field, got %d.\nContent:\n%s", count, content)
	}
}

func TestModelCreation(t *testing.T) {
	tmpDir := setupTestRepo(t)

	cfg, err := config.Load(tmpDir)
	if err != nil {
		t.Fatalf("Failed to load config: %v", err)
	}

	c, err := cache.New(tmpDir)
	if err != nil {
		t.Fatalf("Failed to create cache: %v", err)
	}
	defer c.Close()

	model := New(cfg, tmpDir, c)

	if len(model.views) == 0 {
		t.Fatal("No views created in model")
	}

	if len(model.views) != len(cfg.Views) {
		t.Errorf("Expected %d views, got %d", len(cfg.Views), len(model.views))
	}

	// Each view should have a compiled filter
	for i, vs := range model.views {
		if vs.filter == nil {
			t.Errorf("View %d (%s): filter not compiled", i, vs.cfg.Name)
		}
		if len(vs.subFilters) != len(vs.cfg.Subviews) {
			t.Errorf("View %d (%s): expected %d subfilters, got %d", i, vs.cfg.Name, len(vs.cfg.Subviews), len(vs.subFilters))
		}
	}
}
