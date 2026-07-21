// Package action adapts configured Fennel actions to the existing UI entry points.
package action

import (
	"fmt"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/markdown"
)

type Action struct {
	Name, Description, Filter, Expr string
	config                          config.ActionConfig
}

func Compile(name string, cfg config.ActionConfig) (*Action, error) {
	if cfg.Description == "" {
		return nil, fmt.Errorf("action %q: description is required", name)
	}
	return &Action{Name: name, Description: cfg.Description, Filter: cfg.Filter, Expr: cfg.Expr, config: cfg}, nil
}
func (a *Action) Match(fm map[string]interface{}) bool {
	return a.MatchNote(config.Note{Type: stringValue(fm["type"]), Tags: markdown.ExtractTags(fm), Frontmatter: fm})
}

func (a *Action) MatchNote(note config.Note) bool {
	ok, err := a.config.Match(note)
	return err == nil && ok
}
func (a *Action) Mutate(fm map[string]interface{}) (map[string]interface{}, error) {
	n, err := a.config.Run(config.Note{Path: "action.md", Type: stringValue(fm["type"]), Tags: markdown.ExtractTags(fm), Frontmatter: fm})
	if err != nil {
		return nil, err
	}
	return n.Frontmatter, nil
}

func (a *Action) Run(note config.Note) (config.Note, error) { return a.config.Run(note) }
func (a *Action) Apply(path string, fm map[string]interface{}, body string) error {
	n, err := a.config.Run(config.Note{Path: path, Type: stringValue(fm["type"]), Tags: markdown.ExtractTags(fm), Frontmatter: fm, Body: body})
	if err != nil {
		return err
	}
	return markdown.WriteFrontmatter(path, n.Frontmatter, n.Body)
}
func stringValue(v interface{}) string { s, _ := v.(string); return s }
