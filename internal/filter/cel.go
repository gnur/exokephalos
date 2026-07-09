package filter

import (
	"fmt"
	"sync"

	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/google/cel-go/cel"
)

// Program is a compiled CEL expression ready for evaluation.
type Program struct {
	prg cel.Program
}

// env is the shared CEL environment for all filter compilations.
var (
	sharedEnv  *cel.Env
	envOnce    sync.Once
	envInitErr error
)

func getEnv() (*cel.Env, error) {
	envOnce.Do(func() {
		sharedEnv, envInitErr = cel.NewEnv(
			// Top-level convenience variables
			cel.Variable("type", cel.StringType),
			cel.Variable("tags", cel.ListType(cel.StringType)),
			// Full frontmatter map for accessing any field
			cel.Variable("fm", cel.MapType(cel.StringType, cel.DynType)),
		)
	})
	return sharedEnv, envInitErr
}

// Compile parses and compiles a CEL expression string into a reusable Program.
// The expression has access to:
//   - type (string): the "type" field from frontmatter
//   - tags (list<string>): the "tags" field from frontmatter
//   - fm (map<string, dyn>): the full frontmatter map
func Compile(expr string) (*Program, error) {
	env, err := getEnv()
	if err != nil {
		return nil, fmt.Errorf("initializing CEL environment: %w", err)
	}

	ast, issues := env.Compile(expr)
	if issues.Err() != nil {
		return nil, fmt.Errorf("compiling expression %q: %w", expr, issues.Err())
	}
	if ast.OutputType() != cel.BoolType {
		return nil, fmt.Errorf("expression %q must return a boolean, got %s", expr, ast.OutputType())
	}

	prg, err := env.Program(ast)
	if err != nil {
		return nil, fmt.Errorf("creating program for %q: %w", expr, err)
	}

	return &Program{prg: prg}, nil
}

// Eval evaluates the compiled program against a frontmatter map.
// Returns true if the item matches the filter, false otherwise.
func (p *Program) Eval(frontmatter map[string]interface{}) (bool, error) {
	// Extract type field, default to empty string
	typeVal := ""
	if v, ok := frontmatter["type"]; ok {
		if s, ok := v.(string); ok {
			typeVal = s
		}
	}

	tagsVal := markdown.ExtractTags(frontmatter)
	if tagsVal == nil {
		tagsVal = []string{}
	}

	activation := map[string]any{
		"type": typeVal,
		"tags": tagsVal,
		"fm":   frontmatter,
	}

	out, _, err := p.prg.Eval(activation)
	if err != nil {
		return false, fmt.Errorf("evaluating filter: %w", err)
	}

	result, ok := out.Value().(bool)
	if !ok {
		return false, fmt.Errorf("filter expression did not return a boolean, got %T", out.Value())
	}

	return result, nil
}

// Match is a convenience function that compiles and evaluates in one step.
// Use Compile + Eval for repeated evaluations of the same expression.
func Match(expr string, frontmatter map[string]interface{}) (bool, error) {
	prg, err := Compile(expr)
	if err != nil {
		return false, err
	}
	return prg.Eval(frontmatter)
}
