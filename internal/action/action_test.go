package action

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gnur/exokephalos/internal/config"
)

func TestCompile_Valid(t *testing.T) {
	cfg := config.ActionConfig{
		Filter:      `type == "book" && "reading" in tags`,
		Expr:        `.tags -= ["reading"] | .tags += ["read"]`,
		Description: "Mark as finished reading",
	}
	a, err := Compile("finish-book", cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if a.Name != "finish-book" {
		t.Errorf("name = %q", a.Name)
	}
	if a.Expr != `.tags -= ["reading"] | .tags += ["read"]` {
		t.Errorf("expr = %q", a.Expr)
	}
}

func TestCompile_InvalidCEL(t *testing.T) {
	cfg := config.ActionConfig{
		Filter:      `!!!!bad`,
		Expr:        `.tags -= ["reading"]`,
		Description: "Bad filter",
	}
	_, err := Compile("bad", cfg)
	if err == nil {
		t.Fatal("expected error for invalid CEL filter")
	}
}

func TestCompile_InvalidYQ(t *testing.T) {
	cfg := config.ActionConfig{
		Filter:      `true`,
		Expr:        `!!!bad!!!`,
		Description: "Bad expression",
	}
	_, err := Compile("bad", cfg)
	if err == nil {
		t.Fatal("expected error for invalid yq expression")
	}
}

func TestMatch(t *testing.T) {
	cfg := config.ActionConfig{
		Filter:      `type == "book" && "reading" in tags`,
		Expr:        `.tags -= ["reading"] | .tags += ["read"]`,
		Description: "Mark as finished",
	}
	a, err := Compile("test", cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Should match
	fm := map[string]interface{}{
		"type": "book",
		"tags": []interface{}{"reading", "fiction"},
	}
	if !a.Match(fm) {
		t.Error("expected match for book with reading tag")
	}

	// Should not match - wrong type
	fm2 := map[string]interface{}{
		"type": "note",
		"tags": []interface{}{"reading"},
	}
	if a.Match(fm2) {
		t.Error("expected no match for note")
	}

	// Should not match - missing tag
	fm3 := map[string]interface{}{
		"type": "book",
		"tags": []interface{}{"finished"},
	}
	if a.Match(fm3) {
		t.Error("expected no match for book without reading tag")
	}
}

func TestApply(t *testing.T) {
	cfg := config.ActionConfig{
		Filter:      `true`,
		Expr:        `.tags -= ["reading"] | .tags += ["read"] | .finished = "2025-01-15"`,
		Description: "Mark as finished",
	}
	a, err := Compile("test", cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	dir := t.TempDir()
	path := filepath.Join(dir, "test.md")

	fm := map[string]interface{}{
		"type":  "book",
		"title": "Test Book",
		"tags":  []interface{}{"reading", "fiction"},
	}
	body := "Some content here."

	if err := a.Apply(path, fm, body); err != nil {
		t.Fatalf("Apply failed: %v", err)
	}

	// Read the file back
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("reading file: %v", err)
	}
	content := string(data)

	// Verify tags were changed
	if !strings.Contains(content, "read") {
		t.Error("expected 'read' tag in output")
	}
	if strings.Contains(content, "reading") {
		t.Error("expected 'reading' tag to be removed")
	}
	// Verify new field was added
	if !strings.Contains(content, "finished") {
		t.Error("expected 'finished' field in output")
	}
	// Verify body is preserved
	if !strings.Contains(content, "Some content here") {
		t.Error("expected body content preserved")
	}
}

func TestApply_PreservesBody(t *testing.T) {
	cfg := config.ActionConfig{
		Filter:      `true`,
		Expr:        `.new_field = "added"`,
		Description: "Add field",
	}
	a, err := Compile("test", cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	dir := t.TempDir()
	path := filepath.Join(dir, "test.md")

	fm := map[string]interface{}{
		"type": "note",
	}
	body := "This is the body.\nWith multiple lines.\n"

	if err := a.Apply(path, fm, body); err != nil {
		t.Fatalf("Apply failed: %v", err)
	}

	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("reading file: %v", err)
	}
	content := string(data)

	if !strings.Contains(content, "new_field") {
		t.Error("expected 'new_field' in output")
	}
	if !strings.Contains(content, "This is the body.") {
		t.Error("expected body preserved")
	}
}

func TestApply_NowExpression(t *testing.T) {
	cfg := config.ActionConfig{
		Filter:      `true`,
		Expr:        `.updated = now`,
		Description: "Add timestamp",
	}
	a, err := Compile("test", cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	dir := t.TempDir()
	path := filepath.Join(dir, "test.md")

	fm := map[string]interface{}{
		"type": "note",
	}
	body := "content"

	if err := a.Apply(path, fm, body); err != nil {
		t.Fatalf("Apply failed: %v", err)
	}

	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("reading file: %v", err)
	}
	content := string(data)

	if !strings.Contains(content, "updated") {
		t.Error("expected 'updated' field in output")
	}
}


