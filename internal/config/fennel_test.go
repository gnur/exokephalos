package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestFennelWorkspacePredicatesAndActions(t *testing.T) {
	cfg, err := LoadContents([]NamedContent{{Name: "exo.fnl", Content: []byte(`
{:default-view :notes
 :views {:notes {:name "Notes" :key "n" :when (fn [note] (= note.type "note"))}}
 :actions {:append-body {:description "Append" :run (fn [note] (assoc note :body (.. note.body "!")))}}}
`)}})
	if err != nil {
		t.Fatal(err)
	}
	matched, err := cfg.MatchView("notes", Note{Type: "note", Path: "note.md", Frontmatter: map[string]interface{}{}})
	if err != nil || !matched {
		t.Fatalf("MatchView = %v, %v", matched, err)
	}
	note, err := cfg.RunAction("append-body", Note{Type: "note", Path: "note.md", Tags: []string{}, Frontmatter: map[string]interface{}{}, Body: "body"})
	if err != nil || note.Body != "body!" {
		t.Fatalf("RunAction = %#v, %v", note, err)
	}
}

func TestPermissionsRequireWorkspaceDeclarationAndLocalGrant(t *testing.T) {
	dir := t.TempDir()
	if err := os.Mkdir(filepath.Join(dir, ".exo"), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "input.txt"), []byte("allowed"), 0644); err != nil {
		t.Fatal(err)
	}
	workspace := `{:views {:notes {:name "Notes" :key "n" :when (fn [_] true)}}
:actions {:read {:description "Read" :permissions [:filesystem] :run (fn [note exo] (assoc note :body (exo.filesystem.read "input.txt")))}}}`
	if err := os.WriteFile(filepath.Join(dir, "exo.fnl"), []byte(workspace), 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, ".exo", "permissions.fnl"), []byte(`{:actions {:read {:filesystem {:read ["input.txt"]}}}}`), 0644); err != nil {
		t.Fatal(err)
	}
	cfg, err := Load(dir)
	if err != nil {
		t.Fatal(err)
	}
	note, err := cfg.RunAction("read", Note{Path: "note.md", Type: "note", Frontmatter: map[string]interface{}{}, Tags: []string{}})
	if err != nil || note.Body != "allowed" {
		t.Fatalf("RunAction = %#v, %v", note, err)
	}
	if err := os.WriteFile(filepath.Join(dir, ".exo", "permissions.fnl"), []byte(`{:actions {}}`), 0644); err != nil {
		t.Fatal(err)
	}
	cfg, err = Load(dir)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := cfg.RunAction("read", Note{Path: "note.md", Type: "note", Frontmatter: map[string]interface{}{}, Tags: []string{}}); err == nil {
		t.Fatal("expected ungranted filesystem access to fail")
	}
}

func TestFilesystemWritePermissionIsScoped(t *testing.T) {
	dir := t.TempDir()
	if err := os.Mkdir(filepath.Join(dir, ".exo"), 0755); err != nil {
		t.Fatal(err)
	}
	workspace := `{:views {:notes {:name "Notes" :key "n" :when (fn [_] true)}}
:actions {:write {:description "Write" :permissions [:filesystem] :run (fn [note exo] (do (exo.filesystem.write "generated/output.txt" "written") note))}}}`
	if err := os.WriteFile(filepath.Join(dir, "exo.fnl"), []byte(workspace), 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, ".exo", "permissions.fnl"), []byte(`{:actions {:write {:filesystem {:write ["generated/*"]}}}}`), 0644); err != nil {
		t.Fatal(err)
	}
	cfg, err := Load(dir)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := cfg.RunAction("write", Note{Path: "note.md", Type: "note", Frontmatter: map[string]interface{}{}, Tags: []string{}}); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(filepath.Join(dir, "generated", "output.txt"))
	if err != nil || string(data) != "written" {
		t.Fatalf("write result = %q, %v", data, err)
	}
}

func TestWorkspaceModulesAreSandboxed(t *testing.T) {
	base := []NamedContent{
		{Name: "exo.fnl", Content: []byte(`(local views (require :modules.views)) {:views views :actions {}}`)},
		{Name: "modules/views.fnl", Content: []byte(`{:notes {:name "Notes" :key "n" :when (fn [_] true)}}`)},
	}
	if _, err := LoadContents(base); err != nil {
		t.Fatalf("loading Fennel module: %v", err)
	}
	withLua := []NamedContent{base[0], {Name: "modules/views.lua", Content: []byte(`return { notes = { name = "Notes", key = "n", when = function(_) return true end } }`)}}
	if _, err := LoadContents(withLua); err != nil {
		t.Fatalf("loading Lua module: %v", err)
	}
	missing := []NamedContent{{Name: "exo.fnl", Content: []byte(`(require :modules.missing)`)}}
	if _, err := LoadContents(missing); err == nil {
		t.Fatal("expected missing module error")
	}
	traversal := []NamedContent{{Name: "exo.fnl", Content: []byte(`(require :modules..secret)`)}}
	if _, err := LoadContents(traversal); err == nil {
		t.Fatal("expected traversal module error")
	}
	cycle := []NamedContent{
		{Name: "exo.fnl", Content: []byte(`(require :modules.a)`)},
		{Name: "modules/a.fnl", Content: []byte(`(require :modules.b)`)},
		{Name: "modules/b.fnl", Content: []byte(`(require :modules.a)`)},
	}
	if _, err := LoadContents(cycle); err == nil {
		t.Fatal("expected module cycle error")
	}
	invalidFennel := []NamedContent{{Name: "exo.fnl", Content: []byte(`{:views`)}}
	if _, err := LoadContents(invalidFennel); err == nil {
		t.Fatal("expected invalid Fennel error")
	}
	invalidLua := []NamedContent{{Name: "exo.fnl", Content: []byte(`(require :modules.views)`)}, {Name: "modules/views.lua", Content: []byte(`not valid lua`)}}
	if _, err := LoadContents(invalidLua); err == nil {
		t.Fatal("expected invalid Lua error")
	}
}

func TestExampleWorkspaceCapturesMigratedConfig(t *testing.T) {
	cfg, err := Load(filepath.Join("..", "..", "example-repo"))
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"books", "docs", "notes", "secrets", "webhooks"} {
		if _, ok := cfg.Views[name]; !ok {
			t.Errorf("missing migrated view %q", name)
		}
	}
	for _, name := range []string{"finish-book", "start-book", "mark-done"} {
		if _, ok := cfg.Actions[name]; !ok {
			t.Errorf("missing migrated action %q", name)
		}
	}
}
