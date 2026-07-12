package action

import (
	"fmt"
	"log"
	"strings"

	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/filter"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/mikefarah/yq/v4/pkg/yqlib"
	"gopkg.in/yaml.v3"
)

type Action struct {
	Name        string
	Filter      string
	Expr        string
	Description string
	celProg     *filter.Program
}

func Compile(name string, cfg config.ActionConfig) (*Action, error) {
	var prog *filter.Program
	var err error
	if strings.TrimSpace(cfg.Filter) != "" {
		prog, err = filter.Compile(cfg.Filter)
		if err != nil {
			return nil, fmt.Errorf("action %q: compiling filter: %w", name, err)
		}
	}

	eval := yqlib.NewStringEvaluator()
	_, err = eval.Evaluate(
		cfg.Expr,
		"a: test\ntags: []",
		yqlib.NewYamlEncoder(yqlib.NewDefaultYamlPreferences()),
		yqlib.NewYamlDecoder(yqlib.NewDefaultYamlPreferences()),
	)
	if err != nil {
		return nil, fmt.Errorf("action %q: invalid yq expression: %w", name, err)
	}

	return &Action{
		Name:        name,
		Filter:      cfg.Filter,
		Expr:        cfg.Expr,
		Description: cfg.Description,
		celProg:     prog,
	}, nil
}

func (a *Action) Match(fm map[string]interface{}) bool {
	if a.celProg == nil {
		return true
	}
	ok, err := a.celProg.Eval(fm)
	if err != nil {
		log.Printf("action: failed to evaluate filter for action %s: %v", a.Name, err)
		return false
	}
	return ok
}

func (a *Action) Apply(path string, fm map[string]interface{}, body string) error {
	newFm, err := a.Mutate(fm)
	if err != nil {
		return err
	}
	if err := markdown.WriteFrontmatter(path, newFm, body); err != nil {
		return fmt.Errorf("writing file: %w", err)
	}
	return nil
}

func (a *Action) Mutate(fm map[string]interface{}) (map[string]interface{}, error) {
	yamlBytes, err := yaml.Marshal(fm)
	if err != nil {
		return nil, fmt.Errorf("marshaling frontmatter: %w", err)
	}

	prefs := yqlib.NewDefaultYamlPreferences()
	prefs.PrintDocSeparators = false
	eval := yqlib.NewStringEvaluator()
	result, err := eval.Evaluate(
		a.Expr,
		string(yamlBytes),
		yqlib.NewYamlEncoder(prefs),
		yqlib.NewYamlDecoder(prefs),
	)
	if err != nil {
		return nil, fmt.Errorf("applying yq expression: %w", err)
	}

	var newFm map[string]interface{}
	if err := yaml.Unmarshal([]byte(result), &newFm); err != nil {
		return nil, fmt.Errorf("unmarshaling result: %w", err)
	}

	return newFm, nil
}
