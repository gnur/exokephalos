package config

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"

	"github.com/BurntSushi/toml"
)

// Config is the top-level configuration parsed from .exo.toml.
type Config struct {
	DefaultView string                  `toml:"default_view"`
	Views       map[string]ViewConfig   `toml:"views"`
	Actions     map[string]ActionConfig `toml:"actions"`
}

type AppConfig struct {
	Sync   SyncConfig       `toml:"sync"`
	Server SyncServerConfig `toml:"server"`
}

type SyncConfig struct {
	ServerURL string           `toml:"server_url"`
	ClientID  string           `toml:"client_id"`
	KeyPath   string           `toml:"key_path"`
	Enabled   bool             `toml:"enabled"`
	Server    SyncServerConfig `toml:"server"`
}

type SyncServerConfig struct {
	Enabled bool   `toml:"enabled"`
	DBPath  string `toml:"db_path"`
	Listen  string `toml:"listen"`
}

// ViewConfig defines a single view (e.g., notes, books).
type ViewConfig struct {
	Name            string          `toml:"name" json:"name"`
	Key             string          `toml:"key" json:"key"`
	Filter          string          `toml:"filter" json:"filter"`
	ShowTags        bool            `toml:"show_tags" json:"show_tags"`
	TitleField      string          `toml:"title_field" json:"title_field"`
	SubtitleField   string          `toml:"subtitle_field" json:"subtitle_field"`
	SortField       string          `toml:"sort_field" json:"sort_field"`
	SortOrder       string          `toml:"sort_order" json:"sort_order"`
	Template        string          `toml:"template" json:"template"`
	PreviewTemplate string          `toml:"preview_template" json:"preview_template"`
	StatsTemplate   string          `toml:"stats_template" json:"stats_template"`
	Subviews        []SubviewConfig `toml:"subviews" json:"subviews"`
}

// SubviewConfig defines a subview within a view that narrows the parent filter.
type SubviewConfig struct {
	Name   string `toml:"name" json:"name"`
	Filter string `toml:"filter" json:"filter"`
}

// ActionConfig defines a user-triggered action that transforms an item's frontmatter.
type ActionConfig struct {
	Filter      string `toml:"filter"`
	Expr        string `toml:"expr"`
	Description string `toml:"description"`
}

// OrderedView pairs a view ID with its config for ordered iteration.
type OrderedView struct {
	ID     string
	Config ViewConfig
}

// OrderedViews returns views sorted by their key for deterministic ordering.
func (c *Config) OrderedViews() []OrderedView {
	views := make([]OrderedView, 0, len(c.Views))
	for id, vc := range c.Views {
		views = append(views, OrderedView{ID: id, Config: vc})
	}
	sort.Slice(views, func(i, j int) bool {
		return views[i].Config.Key < views[j].Config.Key
	})
	return views
}

// DefaultViewIndex returns the index of the default view in the ordered views list.
// Returns 0 if no default is set or if the default view ID is not found.
func (c *Config) DefaultViewIndex() int {
	if c.DefaultView == "" {
		return 0
	}
	for i, ov := range c.OrderedViews() {
		if ov.ID == c.DefaultView {
			return i
		}
	}
	return 0
}

// Load reads and parses all configuration files. It looks for all .toml files
// in the .exo/ subdirectory of the given directory and combines them into one configuration.
// If no configuration files are found there, it falls back to loading .exo.toml at the root.
func Load(dir string) (*Config, error) {
	var files []string

	// Preferred synced workspace config: root-level TOML files. Local app
	// config/state lives under .exo and is intentionally not included here.
	entries, _ := os.ReadDir(dir)
	for _, entry := range entries {
		if entry.IsDir() || filepath.Ext(entry.Name()) != ".toml" || entry.Name() == ".exo.toml" {
			continue
		}
		files = append(files, filepath.Join(dir, entry.Name()))
	}

	exoDir := filepath.Join(dir, ".exo")
	// Backward-compatible fallback: legacy view/action config in .exo/.
	if info, err := os.Stat(exoDir); err == nil && info.IsDir() {
		entries, err := os.ReadDir(exoDir)
		if err == nil && len(files) == 0 {
			for _, entry := range entries {
				if !entry.IsDir() && filepath.Ext(entry.Name()) == ".toml" && entry.Name() != "tui.toml" && entry.Name() != "serve.toml" {
					files = append(files, filepath.Join(exoDir, entry.Name()))
				}
			}
		}
	}

	// Fallback to .exo.toml if no config files found in .exo/
	if len(files) == 0 {
		path := filepath.Join(dir, ".exo.toml")
		if _, err := os.Stat(path); err == nil {
			files = append(files, path)
		}
	}

	if len(files) == 0 {
		return nil, fmt.Errorf("no config files found (looked for *.toml in %s and %s)", filepath.Join(dir, ".exo"), filepath.Join(dir, ".exo.toml"))
	}

	// Sort files to ensure deterministic merging
	sort.Strings(files)

	contents := make([]NamedContent, 0, len(files))
	for _, file := range files {
		data, err := os.ReadFile(file)
		if err != nil {
			return nil, fmt.Errorf("reading config file %s: %w", file, err)
		}
		contents = append(contents, NamedContent{Name: file, Content: data})
	}

	return LoadContents(contents)
}

// NamedContent is a TOML config document with a display name used in errors.
type NamedContent struct {
	Name    string
	Content []byte
}

// LoadContents parses and combines config TOML documents in the provided order.
func LoadContents(contents []NamedContent) (*Config, error) {
	combined := Config{
		Views:   make(map[string]ViewConfig),
		Actions: make(map[string]ActionConfig),
	}

	for _, content := range contents {
		var cfg Config
		if err := toml.Unmarshal(content.Content, &cfg); err != nil {
			return nil, fmt.Errorf("parsing config file %s: %w", content.Name, err)
		}

		if cfg.DefaultView != "" {
			combined.DefaultView = cfg.DefaultView
		}
		for id, view := range cfg.Views {
			combined.Views[id] = view
		}
		for name, act := range cfg.Actions {
			combined.Actions[name] = act
		}
	}

	if err := combined.validate(); err != nil {
		return nil, fmt.Errorf("invalid config: %w", err)
	}

	combined.addBuiltInViews()

	// Apply defaults
	for id, v := range combined.Views {
		if v.TitleField == "" {
			v.TitleField = "title"
		}
		if v.SortField == "" {
			v.SortField = "created"
		}
		if v.SortOrder == "" {
			v.SortOrder = "desc"
		}
		if len(v.Subviews) == 0 {
			v.Subviews = []SubviewConfig{{Name: "All", Filter: "true"}}
		}
		combined.Views[id] = v
	}

	return &combined, nil
}

func LoadApp(dir, mode string) (*AppConfig, error) {
	path := filepath.Join(dir, ".exo", mode+".toml")
	cfg := &AppConfig{}
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			applyAppDefaults(dir, cfg)
			return cfg, nil
		}
		return nil, err
	}
	if err := toml.Unmarshal(data, cfg); err != nil {
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

func (c *Config) addBuiltInViews() {
	c.Views["all"] = ViewConfig{
		Name:          "All",
		Key:           "0",
		Filter:        "true",
		ShowTags:      true,
		TitleField:    "title",
		SubtitleField: "type",
		SortField:     "created",
		SortOrder:     "desc",
		Template:      "---\ntype: note\ntags: []\ntitle: \"{{.Title}}\"\n---\n\n",
		Subviews:      []SubviewConfig{{Name: "All", Filter: "true"}},
	}
}

func (c *Config) validate() error {
	if len(c.Views) == 0 {
		return fmt.Errorf("no views defined")
	}

	// Validate default_view references an existing view
	if c.DefaultView != "" {
		if _, ok := c.Views[c.DefaultView]; !ok {
			return fmt.Errorf("default_view %q does not match any defined view", c.DefaultView)
		}
	}

	keys := make(map[string]string) // key -> view ID
	for id, v := range c.Views {
		if v.Name == "" {
			return fmt.Errorf("view %q: name is required", id)
		}
		if v.Key == "" {
			return fmt.Errorf("view %q: key is required", id)
		}
		if v.Filter == "" {
			return fmt.Errorf("view %q: filter is required", id)
		}
		if v.Template == "" {
			return fmt.Errorf("view %q: template is required", id)
		}

		// Check for duplicate keys
		if existing, ok := keys[v.Key]; ok {
			return fmt.Errorf("view %q: key %q conflicts with view %q", id, v.Key, existing)
		}
		keys[v.Key] = id

		// Validate subviews
		for i, sv := range v.Subviews {
			if sv.Name == "" {
				return fmt.Errorf("view %q subview %d: name is required", id, i)
			}
			if sv.Filter == "" {
				return fmt.Errorf("view %q subview %q: filter is required", id, sv.Name)
			}
		}
	}

	for name, a := range c.Actions {
		if a.Expr == "" {
			return fmt.Errorf("action %q: expr is required", name)
		}
		if a.Description == "" {
			return fmt.Errorf("action %q: description is required", name)
		}
	}

	return nil
}
