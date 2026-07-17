package tui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/gnur/exokephalos/internal/action"
	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/filter"
	"github.com/gnur/exokephalos/internal/importer"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/scanner"
)

func TestAppendImportedDescription(t *testing.T) {
	content := "---\ntype: book\n---\n"
	got := appendImportedDescription(content, "A short description.")
	want := "---\ntype: book\n---\n\nA short description.\n"
	if got != want {
		t.Fatalf("content = %q, want %q", got, want)
	}
}

func TestAppendImportedDescriptionPreservesExistingBody(t *testing.T) {
	content := "---\ntype: book\n---\nExisting notes\n"
	got := appendImportedDescription(content, "A short description.")
	want := "---\ntype: book\n---\nExisting notes\n\nA short description.\n"
	if got != want {
		t.Fatalf("content = %q, want %q", got, want)
	}
}

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
	importer.Import("../../example-repo/docs", tmpDir, "doc")

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

func TestActionPickerFiltersByNameAndDescription(t *testing.T) {
	startBook := mustCompileAction(t, "start-book", config.ActionConfig{
		Filter:      `"to-read" in tags`,
		Expr:        `.tags -= ["to-read"] | .tags += ["reading"]`,
		Description: "Start reading this book",
	})
	archive := mustCompileAction(t, "archive-note", config.ActionConfig{
		Expr:        `.tags += ["archived"]`,
		Description: "Archive this note",
	})
	model := Model{
		actions: map[string]*action.Action{
			"start-book":   startBook,
			"archive-note": archive,
		},
		actionInput: textinput.New(),
	}
	model.actionInput.SetValue("start")

	entries := model.filteredActionEntries()
	if len(entries) != 1 {
		t.Fatalf("expected 1 entry, got %d: %#v", len(entries), entries)
	}
	if entries[0].Name != "start-book" {
		t.Fatalf("expected start-book, got %q", entries[0].Name)
	}
}

func TestActionPickerShowsDisabledActionAndReportsFilter(t *testing.T) {
	finish := mustCompileAction(t, "finish-book", config.ActionConfig{
		Filter:      `"reading" in tags`,
		Expr:        `.tags -= ["reading"] | .tags += ["read"]`,
		Description: "Finish book",
	})
	model := Model{
		actions: map[string]*action.Action{"finish-book": finish},
		views: []viewState{{
			items: []scanner.Item{{
				Frontmatter: map[string]interface{}{
					"type": "book",
					"tags": []interface{}{"to-read"},
				},
			}},
			filteredItems: []scanner.Item{{
				Frontmatter: map[string]interface{}{
					"type": "book",
					"tags": []interface{}{"to-read"},
				},
			}},
		}},
		actionInput: textinput.New(),
	}

	entries := model.actionEntries()
	var finishEntry actionEntry
	for _, entry := range entries {
		if entry.Name == "finish-book" {
			finishEntry = entry
			break
		}
	}
	if finishEntry.Name == "" {
		t.Fatal("finish-book entry not found")
	}
	if finishEntry.Enabled {
		t.Fatal("expected finish-book to be disabled")
	}

	updated, _ := model.executeActionEntry(finishEntry)
	result := updated.(Model)
	if result.status != `Requires: "reading" in tags` {
		t.Fatalf("status = %q", result.status)
	}
}

func TestActionPickerSortsEnabledActionsFirst(t *testing.T) {
	disabled := mustCompileAction(t, "aaa-disabled", config.ActionConfig{
		Filter:      `"reading" in tags`,
		Expr:        `.tags += ["read"]`,
		Description: "Disabled action",
	})
	enabled := mustCompileAction(t, "zzz-enabled", config.ActionConfig{
		Expr:        `.tags += ["done"]`,
		Description: "Enabled action",
	})
	model := Model{
		actions: map[string]*action.Action{
			"aaa-disabled": disabled,
			"zzz-enabled":  enabled,
		},
		views: []viewState{{
			filteredItems: []scanner.Item{{
				Frontmatter: map[string]interface{}{
					"type": "book",
					"tags": []interface{}{"to-read"},
				},
			}},
		}},
		actionInput: textinput.New(),
	}

	entries := model.actionEntries()
	var seenDisabled bool
	for _, entry := range entries {
		if !entry.Enabled {
			seenDisabled = true
			continue
		}
		if seenDisabled {
			t.Fatalf("enabled action %q appeared after a disabled action: %#v", entry.Name, entries)
		}
	}
}

func TestActionPickerHardcoverAlwaysEnabled(t *testing.T) {
	model := Model{actionInput: textinput.New()}
	entries := model.actionEntries()
	var hardcover actionEntry
	for _, entry := range entries {
		if entry.Name == "hardcover-search" {
			hardcover = entry
			break
		}
	}
	if hardcover.Name == "" {
		t.Fatal("hardcover-search entry not found")
	}
	if !hardcover.Enabled {
		t.Fatal("expected hardcover-search to be enabled")
	}
	if hardcover.Filter != "" {
		t.Fatalf("expected no hardcover filter requirement, got %q", hardcover.Filter)
	}
}

func TestActionPickerColonOpensPicker(t *testing.T) {
	model := Model{actionInput: textinput.New()}
	updated, _ := model.handleNormalKey(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{':'}})
	result := updated.(Model)
	if result.mode != modeActionPicker {
		t.Fatalf("mode = %v", result.mode)
	}
}

func TestViewShortcutsUseSingleLetterWhenUnique(t *testing.T) {
	model := Model{
		views: []viewState{
			{cfg: config.ViewConfig{Name: "Notes"}},
			{cfg: config.ViewConfig{Name: "Books"}},
		},
	}

	shortcuts := model.viewShortcuts()
	if shortcuts[0].Key != "n" {
		t.Fatalf("notes shortcut = %q, want n", shortcuts[0].Key)
	}
	if shortcuts[1].Key != "b" {
		t.Fatalf("books shortcut = %q, want b", shortcuts[1].Key)
	}
}

func TestViewShortcutsExpandOnFirstLetterConflict(t *testing.T) {
	model := Model{
		views: []viewState{
			{cfg: config.ViewConfig{Name: "All"}},
			{cfg: config.ViewConfig{Name: "Articles"}},
			{cfg: config.ViewConfig{Name: "Books"}},
		},
	}

	shortcuts := model.viewShortcuts()
	keys := map[string]bool{}
	for _, shortcut := range shortcuts {
		if keys[shortcut.Key] {
			t.Fatalf("duplicate shortcut %q in %#v", shortcut.Key, shortcuts)
		}
		keys[shortcut.Key] = true
	}
	if shortcuts[0].Key != "al" {
		t.Fatalf("all shortcut = %q, want al", shortcuts[0].Key)
	}
	if shortcuts[1].Key != "ar" {
		t.Fatalf("articles shortcut = %q, want ar", shortcuts[1].Key)
	}
	if shortcuts[2].Key != "b" {
		t.Fatalf("books shortcut = %q, want b", shortcuts[2].Key)
	}
}

func TestViewMenuAcceptsMultiLetterShortcut(t *testing.T) {
	model := Model{
		mode: modeViewMenu,
		views: []viewState{
			{cfg: config.ViewConfig{Name: "All"}},
			{cfg: config.ViewConfig{Name: "Articles"}},
		},
	}

	updated, _ := model.handleViewMenuKey(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{'a'}})
	result := updated.(Model)
	if result.mode != modeViewMenu {
		t.Fatalf("mode after ambiguous prefix = %v, want modeViewMenu", result.mode)
	}
	if result.viewMenuInput != "a" {
		t.Fatalf("viewMenuInput = %q, want a", result.viewMenuInput)
	}

	updated, _ = result.handleViewMenuKey(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{'r'}})
	result = updated.(Model)
	if result.activeView != 1 {
		t.Fatalf("activeView = %d, want 1", result.activeView)
	}
	if result.mode != modeNormal {
		t.Fatalf("mode = %v, want modeNormal", result.mode)
	}
	if result.viewMenuInput != "" {
		t.Fatalf("viewMenuInput = %q, want empty", result.viewMenuInput)
	}
}

func TestSortViewItemsUsesIDAsDateTieBreaker(t *testing.T) {
	model := Model{}
	vs := viewState{
		cfg: config.ViewConfig{
			SortField: "created",
			SortOrder: "desc",
		},
		items: []scanner.Item{
			{Path: "z.md", Frontmatter: map[string]interface{}{"created": "2026-07-08", "id": "zeta"}},
			{Path: "b.md", Frontmatter: map[string]interface{}{"created": "2026-07-09", "id": "beta"}},
			{Path: "a.md", Frontmatter: map[string]interface{}{"created": "2026-07-09", "id": "alpha"}},
		},
	}

	model.sortViewItems(&vs)

	got := []string{vs.items[0].SortID(), vs.items[1].SortID(), vs.items[2].SortID()}
	want := []string{"alpha", "beta", "zeta"}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("order = %v, want %v", got, want)
		}
	}
}

func mustCompileAction(t *testing.T, name string, cfg config.ActionConfig) *action.Action {
	t.Helper()
	act, err := action.Compile(name, cfg)
	if err != nil {
		t.Fatal(err)
	}
	return act
}
