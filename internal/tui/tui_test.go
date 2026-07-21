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
	"github.com/gnur/exokephalos/internal/encryption"
	"github.com/gnur/exokephalos/internal/hardcover"
	"github.com/gnur/exokephalos/internal/importer"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/scanner"
)

func TestEncryptedEditSavesEditedFrontmatterAndBody(t *testing.T) {
	dir := t.TempDir()
	notePath := filepath.Join(dir, "note.md")
	oldFM := map[string]interface{}{"id": "note123", "title": "Before", "encrypted": true}
	ciphertext, err := encryption.Encrypt("note123", "passphrase", "Before body")
	if err != nil {
		t.Fatal(err)
	}
	if err := markdown.WriteFrontmatter(notePath, oldFM, ciphertext); err != nil {
		t.Fatal(err)
	}

	temp, err := os.CreateTemp(dir, "encrypted-edit-*.md")
	if err != nil {
		t.Fatal(err)
	}
	editedFM := map[string]interface{}{"id": "note123", "title": "After", "category": "private"}
	if err := markdown.WriteFrontmatter(temp.Name(), editedFM, "After body"); err != nil {
		t.Fatal(err)
	}
	if err := temp.Close(); err != nil {
		t.Fatal(err)
	}

	m := Model{
		encryptedEdit: &scanner.Item{Path: notePath, Frontmatter: oldFM, Body: ciphertext},
		encryptedTemp: temp.Name(),
		encryptedPass: "passphrase",
	}
	updated, _ := m.Update(encryptedEditMsg{})
	result := updated.(Model)
	if result.status != "Encrypted note saved" {
		t.Fatalf("status = %q", result.status)
	}

	fm, savedBody, err := markdown.ParseFrontmatter(notePath)
	if err != nil {
		t.Fatal(err)
	}
	if fm["title"] != "After" || fm["category"] != "private" || fm["encrypted"] != true {
		t.Fatalf("frontmatter = %#v", fm)
	}
	plain, err := encryption.Decrypt("note123", "passphrase", savedBody)
	if err != nil {
		t.Fatal(err)
	}
	if plain != "After body" {
		t.Fatalf("decrypted body = %q", plain)
	}
}

func TestAppendImportedDescription(t *testing.T) {
	content := "---\ntype: book\n---\n"
	got := appendImportedDescription(content, "A short description.")
	want := "---\ntype: book\n---\n\nA short description.\n"
	if got != want {
		t.Fatalf("content = %q, want %q", got, want)
	}
}

func TestHardcoverBookTitle(t *testing.T) {
	tests := []struct {
		name string
		book hardcover.Book
		want string
	}{
		{
			name: "numbered series",
			book: hardcover.Book{Title: "Book Title", Series: "Series Name, #1"},
			want: "Book Title (Series Name, #1)",
		},
		{
			name: "series without position",
			book: hardcover.Book{Title: "Book Title", Series: "Series Name"},
			want: "Book Title (Series Name)",
		},
		{
			name: "no series",
			book: hardcover.Book{Title: "Book Title"},
			want: "Book Title",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := hardcoverBookTitle(tt.book); got != tt.want {
				t.Fatalf("hardcoverBookTitle() = %q, want %q", got, tt.want)
			}
		})
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

func TestActionErrorPopupDismisses(t *testing.T) {
	m := Model{
		ready:       true,
		width:       80,
		height:      24,
		views:       []viewState{{}},
		mode:        modeActionError,
		actionError: "intentional action failure",
	}
	if got := m.View(); !strings.Contains(got, "Action failed") || !strings.Contains(got, "intentional action failure") {
		t.Fatalf("error popup = %q", got)
	}
	updated, _ := m.Update(tea.KeyMsg{Type: tea.KeyEnter})
	result := updated.(Model)
	if result.mode != modeNormal || result.actionError != "" {
		t.Fatalf("dismissed popup = mode %v, error %q", result.mode, result.actionError)
	}
}

func setupTestRepo(t *testing.T) string {
	tmpDir := t.TempDir()

	data, err := os.ReadFile(filepath.Join("../../example-repo", "exo.fnl"))
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(tmpDir, "exo.fnl"), data, 0644); err != nil {
		t.Fatal(err)
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

	// Test each view's Fennel predicate matches some items.
	for id, viewCfg := range cfg.Views {
		var matched int
		for _, item := range items {
			ok, err := cfg.MatchView(id, config.Note{ID: item.ID, Path: item.Path, Type: item.Type, Tags: item.Tags, Frontmatter: item.Frontmatter, Body: item.Body})
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
			t.Errorf("View %q: no items matched predicate", id)
		}

		// Test subview filters narrow correctly
		for i, sv := range viewCfg.Subviews {
			var subMatched int
			for _, item := range items {
				note := config.Note{ID: item.ID, Path: item.Path, Type: item.Type, Tags: item.Tags, Frontmatter: item.Frontmatter, Body: item.Body}
				parentOk, _ := cfg.MatchView(id, note)
				if !parentOk {
					continue
				}
				subOk, _ := cfg.MatchSubview(id, i, note)
				if subOk {
					subMatched++
				}
			}

			t.Logf("  Subview %d %q: %d items", i, sv.Name, subMatched)

			// "All" subview should match everything from parent
			if sv.Name == "All" || sv.Name == "all" {
				if subMatched != matched {
					t.Errorf("View %q subview %q: got %d items, expected %d", id, sv.Name, subMatched, matched)
				}
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

	// Predicates are held by config and evaluated during filtering, not copied
	// into Bubble Tea view state.
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
	cfg, err := config.LoadContents([]config.NamedContent{{Name: "exo.fnl", Content: []byte(`{:views {:books {:name "Books" :key "b" :when (fn [_] true)}} :actions {:finish-book {:description "Finish book" :when (fn [note] (var found false) (each [_ tag (ipairs note.tags)] (when (= tag "reading") (set found true))) found) :run (fn [note] note)}}}`)}})
	if err != nil {
		t.Fatal(err)
	}
	finish := mustCompileAction(t, "finish-book", cfg.Actions["finish-book"])
	model := Model{
		cfg:     cfg,
		actions: map[string]*action.Action{"finish-book": finish},
		views: []viewState{{
			items: []scanner.Item{{
				Type: "book", Tags: []string{"to-read"},
				Frontmatter: map[string]interface{}{
					"type": "book",
					"tags": []interface{}{"to-read"},
				},
			}},
			filteredItems: []scanner.Item{{
				Type: "book", Tags: []string{"to-read"},
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
	if result.status != "Action is not applicable to this item" {
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
