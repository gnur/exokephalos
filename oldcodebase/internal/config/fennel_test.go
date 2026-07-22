package config

import (
	"os"
	"path/filepath"
	"testing"
	"time"
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

func TestWorkspaceTagHelpersAndNow(t *testing.T) {
	cfg, err := LoadContents([]NamedContent{{Name: "exo.fnl", Content: []byte(`
{:views {:todos {:name "Todos" :key "t" :when (fn [note] (has-tag note.tags "todo"))}}
 :actions {:complete {:description "Complete" :run (fn [note] (assoc (assoc note :tags (add-tag (remove-tag note.tags "todo") "done")) :updated (now)))}
           :keep-existing {:description "Keep existing" :run (fn [note] (assoc note :tags (add-tag note.tags "todo")))}}}
`)}})
	if err != nil {
		t.Fatal(err)
	}
	original := Note{Type: "note", Path: "note.md", Tags: []string{"todo", "todo", "keep"}, Frontmatter: map[string]interface{}{"tags": []string{"todo", "todo", "keep"}}}
	matched, err := cfg.MatchView("todos", original)
	if err != nil || !matched {
		t.Fatalf("MatchView = %v, %v", matched, err)
	}
	updated, err := cfg.RunAction("complete", original)
	if err != nil {
		t.Fatal(err)
	}
	tags, ok := updated.Frontmatter["tags"].([]interface{})
	if !ok || len(tags) != 2 || tags[0] != "keep" || tags[1] != "done" {
		t.Fatalf("updated tags = %#v", updated.Frontmatter["tags"])
	}
	if got := original.Frontmatter["tags"].([]string); len(got) != 3 || got[0] != "todo" {
		t.Fatalf("input tags were mutated: %#v", got)
	}
	stamp, ok := updated.Frontmatter["updated"].(string)
	if !ok {
		t.Fatalf("updated timestamp = %#v", updated.Frontmatter["updated"])
	}
	if _, err := time.Parse(time.RFC3339, stamp); err != nil || stamp[len(stamp)-1] != 'Z' {
		t.Fatalf("timestamp = %q, %v", stamp, err)
	}
	withoutDuplicate, err := cfg.RunAction("keep-existing", original)
	if err != nil {
		t.Fatal(err)
	}
	tags, ok = withoutDuplicate.Frontmatter["tags"].([]interface{})
	if !ok || len(tags) != 3 || tags[0] != "todo" || tags[1] != "todo" || tags[2] != "keep" {
		t.Fatalf("add-tag duplicated an existing tag: %#v", withoutDuplicate.Frontmatter["tags"])
	}
}

func TestLuaWorkspaceTagHelpers(t *testing.T) {
	cfg, err := LoadContents([]NamedContent{
		{Name: "exo.fnl", Content: []byte(`(local workspace (require :modules.workspace)) workspace`)},
		{Name: "modules/workspace.lua", Content: []byte(`
return {
  views = { todos = { name = "Todos", key = "t", when = function(note) return has_tag(note.tags, "todo") end } },
  actions = { complete = { description = "Complete", run = function(note)
    note.tags = add_tag(remove_tag(note.tags, "todo"), "done")
    note.updated = now()
    return note
  end } }
}

`)},
	})
	if err != nil {
		t.Fatal(err)
	}
	original := Note{Type: "note", Path: "note.md", Tags: []string{"todo", "keep"}, Frontmatter: map[string]interface{}{"tags": []string{"todo", "keep"}}}
	matched, err := cfg.MatchView("todos", original)
	if err != nil || !matched {
		t.Fatalf("MatchView = %v, %v", matched, err)
	}
	updated, err := cfg.RunAction("complete", original)
	if err != nil {
		t.Fatal(err)
	}
	tags, ok := updated.Frontmatter["tags"].([]interface{})
	if !ok || len(tags) != 2 || tags[0] != "keep" || tags[1] != "done" {
		t.Fatalf("updated tags = %#v", updated.Frontmatter["tags"])
	}
}

func TestWorkspaceNotesExposeFrontmatterFieldsDirectly(t *testing.T) {
	cfg, err := LoadContents([]NamedContent{{Name: "exo.fnl", Content: []byte(`
{:views {:todos {:name "Todos" :key "t" :when (fn [note] (= note.status "todo"))}}
 :actions {:complete {:description "Complete" :run (fn [note] (assoc note :status "done"))}}}
`)}})
	if err != nil {
		t.Fatal(err)
	}
	original := Note{Type: "note", Path: "note.md", Tags: []string{}, Frontmatter: map[string]interface{}{"id": "note", "type": "note", "status": "todo"}}
	matched, err := cfg.MatchView("todos", original)
	if err != nil || !matched {
		t.Fatalf("MatchView = %v, %v", matched, err)
	}
	updated, err := cfg.RunAction("complete", original)
	if err != nil || updated.Frontmatter["status"] != "done" {
		t.Fatalf("RunAction = %#v, %v", updated, err)
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
