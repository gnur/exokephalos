package filter

import (
	"testing"
)

func TestCompile_Valid(t *testing.T) {
	prg, err := Compile(`type == "note"`)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if prg == nil {
		t.Fatal("program is nil")
	}
}

func TestCompile_Invalid(t *testing.T) {
	_, err := Compile(`this is not valid CEL !!!`)
	if err == nil {
		t.Fatal("expected error for invalid expression, got nil")
	}
}

func TestEval_TypeMatch(t *testing.T) {
	prg, err := Compile(`type == "note"`)
	if err != nil {
		t.Fatalf("compile: %v", err)
	}

	result, err := prg.Eval(map[string]interface{}{"type": "note"})
	if err != nil {
		t.Fatalf("eval: %v", err)
	}
	if !result {
		t.Error("expected true for type == note")
	}
}

func TestEval_TypeMismatch(t *testing.T) {
	prg, err := Compile(`type == "note"`)
	if err != nil {
		t.Fatalf("compile: %v", err)
	}

	result, err := prg.Eval(map[string]interface{}{"type": "article"})
	if err != nil {
		t.Fatalf("eval: %v", err)
	}
	if result {
		t.Error("expected false for type == article")
	}
}

func TestEval_TagsMembership(t *testing.T) {
	prg, err := Compile(`"read" in tags`)
	if err != nil {
		t.Fatalf("compile: %v", err)
	}

	result, err := prg.Eval(map[string]interface{}{
		"tags": []interface{}{"read", "fiction"},
	})
	if err != nil {
		t.Fatalf("eval: %v", err)
	}
	if !result {
		t.Error("expected true for 'read' in tags")
	}
}

func TestEval_TagsNonMembership(t *testing.T) {
	prg, err := Compile(`"read" in tags`)
	if err != nil {
		t.Fatalf("compile: %v", err)
	}

	result, err := prg.Eval(map[string]interface{}{
		"tags": []interface{}{"to-read"},
	})
	if err != nil {
		t.Fatalf("eval: %v", err)
	}
	if result {
		t.Error("expected false for 'read' not in tags")
	}
}

func TestEval_Combined(t *testing.T) {
	prg, err := Compile(`type == "note" && "read" in tags`)
	if err != nil {
		t.Fatalf("compile: %v", err)
	}

	// Both match
	result, err := prg.Eval(map[string]interface{}{
		"type": "note",
		"tags": []interface{}{"read"},
	})
	if err != nil {
		t.Fatalf("eval: %v", err)
	}
	if !result {
		t.Error("expected true for combined match")
	}

	// Type matches but tags don't
	result, err = prg.Eval(map[string]interface{}{
		"type": "note",
		"tags": []interface{}{"unread"},
	})
	if err != nil {
		t.Fatalf("eval: %v", err)
	}
	if result {
		t.Error("expected false when tags don't match")
	}

	// Tags match but type doesn't
	result, err = prg.Eval(map[string]interface{}{
		"type": "article",
		"tags": []interface{}{"read"},
	})
	if err != nil {
		t.Fatalf("eval: %v", err)
	}
	if result {
		t.Error("expected false when type doesn't match")
	}
}

func TestEval_MissingType(t *testing.T) {
	prg, err := Compile(`type == ""`)
	if err != nil {
		t.Fatalf("compile: %v", err)
	}

	result, err := prg.Eval(map[string]interface{}{})
	if err != nil {
		t.Fatalf("eval: %v", err)
	}
	if !result {
		t.Error("expected true: missing type should default to empty string")
	}
}

func TestEval_MissingTags(t *testing.T) {
	prg, err := Compile(`size(tags) == 0`)
	if err != nil {
		t.Fatalf("compile: %v", err)
	}

	result, err := prg.Eval(map[string]interface{}{})
	if err != nil {
		t.Fatalf("eval: %v", err)
	}
	if !result {
		t.Error("expected true: missing tags should default to empty list")
	}
}

func TestEval_TrueExpression(t *testing.T) {
	prg, err := Compile(`true`)
	if err != nil {
		t.Fatalf("compile: %v", err)
	}

	result, err := prg.Eval(map[string]interface{}{"type": "anything"})
	if err != nil {
		t.Fatalf("eval: %v", err)
	}
	if !result {
		t.Error("expected true for literal true expression")
	}
}

func TestEval_Negation(t *testing.T) {
	prg, err := Compile(`!("read" in tags)`)
	if err != nil {
		t.Fatalf("compile: %v", err)
	}

	// Without the tag -> true
	result, err := prg.Eval(map[string]interface{}{
		"tags": []interface{}{"fiction"},
	})
	if err != nil {
		t.Fatalf("eval: %v", err)
	}
	if !result {
		t.Error("expected true: 'read' not in tags")
	}

	// With the tag -> false
	result, err = prg.Eval(map[string]interface{}{
		"tags": []interface{}{"read", "fiction"},
	})
	if err != nil {
		t.Fatalf("eval: %v", err)
	}
	if result {
		t.Error("expected false: 'read' is in tags")
	}
}

func TestMatch_Convenience(t *testing.T) {
	result, err := Match(`type == "note"`, map[string]interface{}{"type": "note"})
	if err != nil {
		t.Fatalf("match: %v", err)
	}
	if !result {
		t.Error("expected true")
	}

	result, err = Match(`type == "note"`, map[string]interface{}{"type": "book"})
	if err != nil {
		t.Fatalf("match: %v", err)
	}
	if result {
		t.Error("expected false")
	}
}

func TestMatch_InvalidExpression(t *testing.T) {
	_, err := Match(`not valid!!!`, map[string]interface{}{})
	if err == nil {
		t.Fatal("expected error for invalid expression")
	}
}
