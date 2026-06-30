package config

import (
	"os"
	"path/filepath"
	"testing"
)

const validConfig = `
[views.notes]
name = "Notes"
key = "n"
filter = 'type == "note"'
path_template = "notes/{{.ID}}.md"
template = "---\ntype: note\n---\n"
`

func writeConfig(t *testing.T, dir, content string) {
	t.Helper()
	err := os.WriteFile(filepath.Join(dir, ".exo.toml"), []byte(content), 0644)
	if err != nil {
		t.Fatalf("writing config: %v", err)
	}
}

func TestLoad_ValidConfig(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, validConfig)

	cfg, err := Load(dir)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if cfg == nil {
		t.Fatal("config is nil")
	}
	v, ok := cfg.Views["notes"]
	if !ok {
		t.Fatal("expected 'notes' view")
	}
	if v.Name != "Notes" {
		t.Errorf("name = %q, want %q", v.Name, "Notes")
	}
	if v.Key != "n" {
		t.Errorf("key = %q, want %q", v.Key, "n")
	}
	if v.Filter != `type == "note"` {
		t.Errorf("filter = %q, want %q", v.Filter, `type == "note"`)
	}
}

func TestLoad_MissingFile(t *testing.T) {
	dir := t.TempDir()
	_, err := Load(dir)
	if err == nil {
		t.Fatal("expected error for missing file, got nil")
	}
}

func TestLoad_MissingName(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, `
[views.notes]
key = "n"
filter = 'type == "note"'
path_template = "notes/{{.ID}}.md"
template = "---\ntype: note\n---\n"
`)
	_, err := Load(dir)
	if err == nil {
		t.Fatal("expected validation error for missing name")
	}
}

func TestLoad_MissingKey(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, `
[views.notes]
name = "Notes"
filter = 'type == "note"'
path_template = "notes/{{.ID}}.md"
template = "---\ntype: note\n---\n"
`)
	_, err := Load(dir)
	if err == nil {
		t.Fatal("expected validation error for missing key")
	}
}

func TestLoad_MissingFilter(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, `
[views.notes]
name = "Notes"
key = "n"
path_template = "notes/{{.ID}}.md"
template = "---\ntype: note\n---\n"
`)
	_, err := Load(dir)
	if err == nil {
		t.Fatal("expected validation error for missing filter")
	}
}

func TestLoad_MissingTemplate(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, `
[views.notes]
name = "Notes"
key = "n"
filter = 'type == "note"'
path_template = "notes/{{.ID}}.md"
`)
	_, err := Load(dir)
	if err == nil {
		t.Fatal("expected validation error for missing template")
	}
}



func TestLoad_DuplicateKeys(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, `
[views.notes]
name = "Notes"
key = "n"
filter = 'type == "note"'
path_template = "notes/{{.ID}}.md"
template = "---\ntype: note\n---\n"

[views.novels]
name = "Novels"
key = "n"
filter = 'type == "novel"'
path_template = "novels/{{.ID}}.md"
template = "---\ntype: novel\n---\n"
`)
	_, err := Load(dir)
	if err == nil {
		t.Fatal("expected validation error for duplicate keys")
	}
}

func TestLoad_DefaultsApplied(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, validConfig)

	cfg, err := Load(dir)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	v := cfg.Views["notes"]

	if v.TitleField != "title" {
		t.Errorf("title_field = %q, want %q", v.TitleField, "title")
	}
	if v.SortField != "created" {
		t.Errorf("sort_field = %q, want %q", v.SortField, "created")
	}
	if v.SortOrder != "desc" {
		t.Errorf("sort_order = %q, want %q", v.SortOrder, "desc")
	}
	if len(v.Subviews) != 1 {
		t.Fatalf("subviews len = %d, want 1", len(v.Subviews))
	}
	if v.Subviews[0].Name != "All" {
		t.Errorf("default subview name = %q, want %q", v.Subviews[0].Name, "All")
	}
	if v.Subviews[0].Filter != "true" {
		t.Errorf("default subview filter = %q, want %q", v.Subviews[0].Filter, "true")
	}
}

func TestLoad_DefaultsNotOverridden(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, `
[views.notes]
name = "Notes"
key = "n"
filter = 'type == "note"'
path_template = "notes/{{.ID}}.md"
template = "---\ntype: note\n---\n"
title_field = "heading"
sort_field = "updated"
sort_order = "asc"

[[views.notes.subviews]]
name = "Recent"
filter = "true"
`)
	cfg, err := Load(dir)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	v := cfg.Views["notes"]
	if v.TitleField != "heading" {
		t.Errorf("title_field = %q, want %q", v.TitleField, "heading")
	}
	if v.SortField != "updated" {
		t.Errorf("sort_field = %q, want %q", v.SortField, "updated")
	}
	if v.SortOrder != "asc" {
		t.Errorf("sort_order = %q, want %q", v.SortOrder, "asc")
	}
	if len(v.Subviews) != 1 || v.Subviews[0].Name != "Recent" {
		t.Errorf("subviews should not be overridden, got %+v", v.Subviews)
	}
}

func TestOrderedViews_SortedByKey(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, `
[views.books]
name = "Books"
key = "b"
filter = 'type == "book"'
path_template = "books/{{.ID}}.md"
template = "---\ntype: book\n---\n"

[views.notes]
name = "Notes"
key = "n"
filter = 'type == "note"'
path_template = "notes/{{.ID}}.md"
template = "---\ntype: note\n---\n"

[views.archives]
name = "Archives"
key = "a"
filter = 'type == "archive"'
path_template = "archives/{{.ID}}.md"
template = "---\ntype: archive\n---\n"
`)

	cfg, err := Load(dir)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	views := cfg.OrderedViews()
	if len(views) != 3 {
		t.Fatalf("expected 3 views, got %d", len(views))
	}

	expectedKeys := []string{"a", "b", "n"}
	for i, v := range views {
		if v.Config.Key != expectedKeys[i] {
			t.Errorf("views[%d].Key = %q, want %q", i, v.Config.Key, expectedKeys[i])
		}
	}
}

func TestDefaultViewIndex_Set(t *testing.T) {
	dir := t.TempDir()
	config := `
default_view = "books"

[views.notes]
name = "Notes"
key = "n"
filter = 'type == "note"'
path_template = "notes/{{.ID}}.md"
template = "---\ntype: note\n---\n"

[views.books]
name = "Books"
key = "b"
filter = 'type == "book"'
path_template = "books/{{.Slug}}.md"
template = "---\ntype: book\n---\n"
`
	writeConfig(t, dir, config)

	cfg, err := Load(dir)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	idx := cfg.DefaultViewIndex()
	views := cfg.OrderedViews()
	if views[idx].ID != "books" {
		t.Errorf("DefaultViewIndex() pointed to %q, want %q", views[idx].ID, "books")
	}
}

func TestDefaultViewIndex_Unset(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, validConfig)

	cfg, err := Load(dir)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if cfg.DefaultViewIndex() != 0 {
		t.Errorf("DefaultViewIndex() = %d, want 0 when unset", cfg.DefaultViewIndex())
	}
}

func TestLoad_ValidActions(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, validConfig+`
[actions.finish-book]
filter = 'type == "book" && "reading" in tags'
expr = '.tags -= ["reading"] | .tags += ["read"] | .finished = now'
description = "Mark as finished reading"
`)
	cfg, err := Load(dir)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	a, ok := cfg.Actions["finish-book"]
	if !ok {
		t.Fatal("expected 'finish-book' action")
	}
	if a.Filter != `type == "book" && "reading" in tags` {
		t.Errorf("filter = %q", a.Filter)
	}
	if a.Expr != `.tags -= ["reading"] | .tags += ["read"] | .finished = now` {
		t.Errorf("expr = %q", a.Expr)
	}
	if a.Description != "Mark as finished reading" {
		t.Errorf("description = %q", a.Description)
	}
}

func TestLoad_ActionMissingFilter(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, validConfig+`
[actions.bad-action]
expr = '.foo = "bar"'
description = "Bad action"
`)
	_, err := Load(dir)
	if err == nil {
		t.Fatal("expected validation error for action missing filter")
	}
}

func TestLoad_ActionMissingExpr(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, validConfig+`
[actions.bad-action]
filter = 'true'
description = "Bad action"
`)
	_, err := Load(dir)
	if err == nil {
		t.Fatal("expected validation error for action missing expr")
	}
}

func TestLoad_ActionMissingDescription(t *testing.T) {
	dir := t.TempDir()
	writeConfig(t, dir, validConfig+`
[actions.bad-action]
filter = 'true'
expr = '.foo = "bar"'
`)
	_, err := Load(dir)
	if err == nil {
		t.Fatal("expected validation error for action missing description")
	}
}

func TestDefaultViewIndex_Invalid(t *testing.T) {
	dir := t.TempDir()
	config := `
default_view = "nonexistent"

[views.notes]
name = "Notes"
key = "n"
filter = 'type == "note"'
path_template = "notes/{{.ID}}.md"
template = "---\ntype: note\n---\n"
`
	writeConfig(t, dir, config)

	_, err := Load(dir)
	if err == nil {
		t.Fatal("expected error for invalid default_view, got nil")
	}
}

func TestLoad_MultipleConfigFiles(t *testing.T) {
	dir := t.TempDir()
	exoDir := filepath.Join(dir, ".exo")
	if err := os.MkdirAll(exoDir, 0755); err != nil {
		t.Fatal(err)
	}

	// File 1: view notes
	viewContent := `
[views.notes]
name = "Notes"
key = "n"
filter = 'type == "note"'
path_template = "notes/{{.ID}}.md"
template = "---\ntype: note\n---\n"
`
	if err := os.WriteFile(filepath.Join(exoDir, "notes.toml"), []byte(viewContent), 0644); err != nil {
		t.Fatal(err)
	}

	// File 2: action and default view
	actionContent := `
default_view = "notes"

[actions.finish-book]
filter = 'type == "book" && "reading" in tags'
expr = '.tags -= ["reading"] | .tags += ["read"] | .finished = now'
description = "Mark as finished reading"
`
	if err := os.WriteFile(filepath.Join(exoDir, "actions.toml"), []byte(actionContent), 0644); err != nil {
		t.Fatal(err)
	}

	cfg, err := Load(dir)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if cfg.DefaultView != "notes" {
		t.Errorf("default_view = %q, want %q", cfg.DefaultView, "notes")
	}

	if _, ok := cfg.Views["notes"]; !ok {
		t.Error("expected 'notes' view to be parsed")
	}

	if _, ok := cfg.Actions["finish-book"]; !ok {
		t.Error("expected 'finish-book' action to be parsed")
	}
}
