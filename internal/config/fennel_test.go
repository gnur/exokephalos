package config

import "testing"

func TestFennelWorkspacePredicatesAndActions(t *testing.T) {
	cfg, err := LoadContents([]NamedContent{{Name: "exo.fnl", Content: []byte(`
{:default-view :notes
 :views {:notes {:name "Notes" :key "n" :when (fn [note] (= note.type "note")) :template "---\n---\n"}}
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
