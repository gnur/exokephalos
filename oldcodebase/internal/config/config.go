// Package config loads the sandboxed Fennel/Lua workspace configuration.
package config

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

type Config struct {
	DefaultView string
	Views       map[string]ViewConfig
	Actions     map[string]ActionConfig
	runtime     *runtime
	baseDir     string
}

type AppConfig struct {
	Sync   SyncConfig
	Server SyncServerConfig
}
type SyncConfig struct {
	ServerURL, ClientID, KeyPath string
	Enabled                      bool
	Server                       SyncServerConfig
}
type SyncServerConfig struct {
	Enabled        bool
	DBPath, Listen string
}

type ViewConfig struct {
	Name string `json:"name"`
	Key  string `json:"key"`
	// Filter is retained only as an empty display field while callers migrate;
	// workspace filtering is always the compiled :when function.
	Filter          string          `json:"filter"`
	ShowTags        bool            `json:"show_tags"`
	TitleField      string          `json:"title_field"`
	SubtitleField   string          `json:"subtitle_field"`
	SortField       string          `json:"sort_field"`
	SortOrder       string          `json:"sort_order"`
	PreviewTemplate string          `json:"preview_template"`
	StatsTemplate   string          `json:"stats_template"`
	Subviews        []SubviewConfig `json:"subviews"`
	when            callable
}
type SubviewConfig struct {
	Name   string `json:"name"`
	Filter string `json:"filter"`
	when   callable
}
type ActionConfig struct {
	Description  string
	Filter, Expr string
	Permissions  []string
	when, run    callable
	runtime      *runtime
	name         string
	grant        PermissionGrant
}
type PermissionGrant struct{ Read, Write, Origins []string }
type OrderedView struct {
	ID     string
	Config ViewConfig
}

// NamedContent is one member of a virtual workspace. Names are slash-separated,
// relative paths such as exo.fnl or modules/views.fnl.
type NamedContent struct {
	Name    string
	Content []byte
}

func IsWorkspacePath(name string) bool {
	name = filepath.ToSlash(filepath.Clean(name))
	return name == "exo.fnl" || (strings.HasPrefix(name, "modules/") && (strings.HasSuffix(name, ".fnl") || strings.HasSuffix(name, ".lua")))
}

func (c *Config) OrderedViews() []OrderedView {
	views := make([]OrderedView, 0, len(c.Views))
	for id, vc := range c.Views {
		views = append(views, OrderedView{ID: id, Config: vc})
	}
	sort.Slice(views, func(i, j int) bool { return views[i].Config.Key < views[j].Config.Key })
	return views
}
func (c *Config) DefaultViewIndex() int {
	for i, v := range c.OrderedViews() {
		if c.DefaultView != "" && v.ID == c.DefaultView {
			return i
		}
	}
	return 0
}

// Load reads the required exo.fnl entrypoint and optional modules beneath the
// workspace. Files below .exo are always host-local and are never loaded here.
func Load(dir string) (*Config, error) {
	entry := filepath.Join(dir, "exo.fnl")
	if _, err := os.Stat(entry); err != nil {
		if os.IsNotExist(err) {
			return nil, fmt.Errorf("no workspace configuration found: create %s", entry)
		}
		return nil, err
	}
	var contents []NamedContent
	err := filepath.WalkDir(filepath.Join(dir, "modules"), func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			return nil
		}
		rel, err := filepath.Rel(dir, path)
		if err != nil {
			return err
		}
		if ext := filepath.Ext(rel); ext != ".fnl" && ext != ".lua" {
			return nil
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		contents = append(contents, NamedContent{Name: filepath.ToSlash(rel), Content: data})
		return nil
	})
	if err != nil && !os.IsNotExist(err) {
		return nil, err
	}
	data, err := os.ReadFile(entry)
	if err != nil {
		return nil, err
	}
	contents = append(contents, NamedContent{Name: "exo.fnl", Content: data})
	cfg, err := LoadContents(contents)
	if err != nil {
		return nil, err
	}
	cfg.baseDir = dir
	cfg.runtime.baseDir = dir
	return cfg, LoadPermissions(dir, cfg)
}

// LoadContents validates and loads a complete virtual workspace module graph.
func LoadContents(contents []NamedContent) (*Config, error) { return loadWorkspace(contents) }

func LoadApp(dir, mode string) (*AppConfig, error) {
	cfg := &AppConfig{}
	path := filepath.Join(dir, ".exo", mode+".fnl")
	if data, err := os.ReadFile(path); err == nil {
		if err := loadAppTable(data, path, cfg); err != nil {
			return nil, err
		}
	} else if !os.IsNotExist(err) {
		return nil, err
	}
	applyAppDefaults(dir, cfg)
	return cfg, nil
}
func applyAppDefaults(dir string, cfg *AppConfig) {
	if cfg.Sync.Server.Enabled || cfg.Sync.Server.DBPath != "" || cfg.Sync.Server.Listen != "" {
		cfg.Server = cfg.Sync.Server
	}
	if cfg.Sync.KeyPath == "" {
		cfg.Sync.KeyPath = filepath.Join(dir, ".exo", "keys", "client_ed25519")
	}
	if cfg.Server.DBPath == "" {
		cfg.Server.DBPath = filepath.Join(dir, ".exo", "server.sqlite")
	} else if !filepath.IsAbs(cfg.Server.DBPath) {
		cfg.Server.DBPath = filepath.Join(dir, cfg.Server.DBPath)
	}
	if cfg.Server.Listen == "" {
		cfg.Server.Listen = ":8293"
	}
}

func (c *Config) MatchView(id string, note Note) (bool, error) {
	v, ok := c.Views[id]
	if !ok {
		return false, fmt.Errorf("unknown view %q", id)
	}
	return c.runtime.callPredicate(v.when, note)
}
func (c *Config) MatchSubview(view string, index int, note Note) (bool, error) {
	v, ok := c.Views[view]
	if !ok || index < 0 || index >= len(v.Subviews) {
		return false, fmt.Errorf("unknown subview")
	}
	return c.runtime.callPredicate(v.Subviews[index].when, note)
}
func (c *Config) MatchAction(name string, note Note) (bool, error) {
	a, ok := c.Actions[name]
	if !ok {
		return false, fmt.Errorf("unknown action %q", name)
	}
	return a.Match(note)
}
func (c *Config) RunAction(name string, note Note) (Note, error) {
	a, ok := c.Actions[name]
	if !ok {
		return Note{}, fmt.Errorf("unknown action %q", name)
	}
	return a.Run(note)
}
func (a ActionConfig) Match(note Note) (bool, error) {
	if a.when.fn == nil {
		return true, nil
	}
	return a.runtime.callPredicate(a.when, note)
}
func (a ActionConfig) Run(note Note) (Note, error) {
	if a.runtime == nil {
		return Note{}, fmt.Errorf("action %q is not attached to a loaded Fennel workspace", a.name)
	}
	return a.runtime.callAction(a.run, note, a.runtime.capabilities(a.grant))
}

func LoadPermissions(dir string, cfg *Config) error {
	data, err := os.ReadFile(filepath.Join(dir, ".exo", "permissions.fnl"))
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		return err
	}
	r, err := newRuntime([]NamedContent{{Name: "exo.fnl", Content: []byte("{}")}})
	if err != nil {
		return err
	}
	defer r.L.Close()
	v, err := r.execute(string(data), "permissions.fnl")
	if err != nil {
		return err
	}
	return applyPermissions(cfg, v)
}

func (c *Config) addBuiltInViews() {
	c.Views["all"] = ViewConfig{Name: "All", Key: "0", ShowTags: true, TitleField: "title", SubtitleField: "type", SortField: "created", SortOrder: "desc", Subviews: []SubviewConfig{{Name: "All", when: alwaysCallable(c.runtime)}}, when: alwaysCallable(c.runtime)}
}
func (c *Config) validate() error {
	if len(c.Views) == 0 {
		return fmt.Errorf("no views defined")
	}
	if c.DefaultView != "" {
		if _, ok := c.Views[c.DefaultView]; !ok {
			return fmt.Errorf(":default-view %q does not match a view", c.DefaultView)
		}
	}
	keys := map[string]string{}
	for id, v := range c.Views {
		if v.Name == "" || v.Key == "" || v.when.fn == nil {
			return fmt.Errorf("view %q requires :name, :key, and function :when", id)
		}
		if prior, ok := keys[v.Key]; ok {
			return fmt.Errorf("view %q key %q conflicts with %q", id, v.Key, prior)
		}
		keys[v.Key] = id
		for _, sv := range v.Subviews {
			if sv.Name == "" || sv.when.fn == nil {
				return fmt.Errorf("view %q subview requires :name and function :when", id)
			}
		}
	}
	for name, a := range c.Actions {
		if a.Description == "" || a.run.fn == nil {
			return fmt.Errorf("action %q requires :description and function :run", name)
		}
	}
	return nil
}
