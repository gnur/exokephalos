package handlers

import (
	"bytes"
	"context"
	"fmt"
	"html/template"
	"io/fs"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"time"

	"github.com/gnur/exokephalos/internal/action"
	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/filter"
	"github.com/gnur/exokephalos/internal/repo"
	"github.com/gnur/exokephalos/internal/scanner"
	"github.com/gnur/exokephalos/internal/syncsvc"
	"github.com/gnur/exokephalos/internal/version"
	"github.com/microcosm-cc/bluemonday"
	"github.com/yuin/goldmark"
)

var wikiLinkRegex = regexp.MustCompile(`\[\[\s*([a-zA-Z0-9]+)\s*\]\]`)

type contextKey string

const requestStartKey contextKey = "requestStart"

// Handlers serves the web interface using config-driven views.
type Handlers struct {
	Cfg        *config.Config
	BaseDir    string
	Repo       *repo.Repo
	Cache      *cache.Cache
	Store      ItemStore
	SyncServer *syncsvc.Server
	templateFS fs.FS
	funcMap    template.FuncMap
	hostname   string
	// Compiled CEL filters per view ID.
	filters map[string]*filter.Program
	// Compiled CEL filters per subview, keyed by "viewID\x00subviewName".
	subviewFilters map[string]*filter.Program
	// Compiled actions from config
	actions map[string]*action.Action
	// Pre-parsed templates keyed by page name (e.g. "views/list.html").
	templates map[string]*template.Template

	cfgMu           sync.RWMutex
	configChanged   bool
	configChangedMu sync.Mutex
	lastConfigCheck time.Time
	configFiles     map[string]time.Time
}

func New(cfg *config.Config, baseDir string, r *repo.Repo, c *cache.Cache, templateFS fs.FS) (*Handlers, error) {
	hostname, _ := os.Hostname()
	h := &Handlers{
		Cfg:            cfg,
		BaseDir:        baseDir,
		Repo:           r,
		Cache:          c,
		Store:          newFilesystemStore(r, c),
		templateFS:     templateFS,
		hostname:       hostname,
		filters:        make(map[string]*filter.Program),
		subviewFilters: make(map[string]*filter.Program),
		templates:      make(map[string]*template.Template),
		funcMap: template.FuncMap{
			"join": strings.Join,
			"list": func(args ...string) []string {
				return args
			},
			"contains": func(slice []string, item string) bool {
				for _, s := range slice {
					if s == item {
						return true
					}
				}
				return false
			},
			"tof":  func(i int) float64 { return float64(i) },
			"mulf": func(a, b float64) float64 { return a * b },
			"divf": func(a, b float64) float64 {
				if b == 0 {
					return 0
				}
				return a / b
			},
			"fmtVal": func(v interface{}) string {
				if v == nil {
					return ""
				}
				switch val := v.(type) {
				case string:
					return val
				case []interface{}:
					parts := make([]string, 0, len(val))
					for _, item := range val {
						parts = append(parts, fmt.Sprintf("%v", item))
					}
					return strings.Join(parts, ", ")
				case map[string]interface{}:
					parts := make([]string, 0, len(val))
					for k, v := range val {
						parts = append(parts, fmt.Sprintf("%s: %v", k, v))
					}
					return strings.Join(parts, ", ")
				case map[interface{}]interface{}:
					parts := make([]string, 0, len(val))
					for k, v := range val {
						parts = append(parts, fmt.Sprintf("%v: %v", k, v))
					}
					return strings.Join(parts, ", ")
				default:
					return fmt.Sprintf("%v", val)
				}
			},
			"itemTitle": func(item scanner.Item, field string) string {
				return item.Title(field)
			},
			"itemSubtitle": func(item scanner.Item, field string) string {
				return item.Subtitle(field)
			},
			"itemTags": func(item scanner.Item) []string {
				return item.GetTags()
			},
			"itemFm": func(item scanner.Item, field string) string {
				v, ok := item.Frontmatter[field]
				if !ok {
					return ""
				}
				switch val := v.(type) {
				case time.Time:
					return val.Format("2006-01-02")
				case string:
					return val
				default:
					return fmt.Sprintf("%v", v)
				}
			},
			"itemYear": func(item scanner.Item, field string) string {
				return item.Year(field)
			},
			"itemID": func(item scanner.Item) string {
				if id, ok := item.Frontmatter["id"].(string); ok {
					return id
				}
				// Filename fallback
				base := item.Path[strings.LastIndex(item.Path, "/")+1:]
				return strings.TrimSuffix(base, ".md")
			},
			"toggleViewTag": func(viewID string, activeTags []string, tag string) string {
				var newTags []string
				found := false
				for _, t := range activeTags {
					if t == tag {
						found = true
					} else {
						newTags = append(newTags, t)
					}
				}
				if !found {
					newTags = append(newTags, tag)
				}
				if len(newTags) == 0 {
					return "/views/" + viewID
				}
				return "/views/" + viewID + "?tags=" + strings.Join(newTags, ",")
			},
			// Legacy: used by old zettelkasten templates
			"toggleTag": func(activeTags []string, tag string) string {
				var newTags []string
				found := false
				for _, t := range activeTags {
					if t == tag {
						found = true
					} else {
						newTags = append(newTags, t)
					}
				}
				if !found {
					newTags = append(newTags, tag)
				}
				if len(newTags) == 0 {
					return "/zettelkasten"
				}
				return "/zettelkasten?tags=" + strings.Join(newTags, ",")
			},
		},
	}

	// Compile CEL filters for each view
	for id, vc := range cfg.Views {
		prog, err := filter.Compile(vc.Filter)
		if err != nil {
			return nil, fmt.Errorf("view %q: compiling filter: %w", id, err)
		}
		h.filters[id] = prog

		// Pre-compile subview filters
		for _, sv := range vc.Subviews {
			if sv.Filter == "" || sv.Filter == "true" {
				continue
			}
			svProg, err := filter.Compile(sv.Filter)
			if err != nil {
				return nil, fmt.Errorf("view %q subview %q: compiling filter: %w", id, sv.Name, err)
			}
			h.subviewFilters[id+"\x00"+sv.Name] = svProg
		}
	}

	// Compile actions from config
	h.actions = make(map[string]*action.Action)
	for name, ac := range cfg.Actions {
		act, err := action.Compile(name, ac)
		if err != nil {
			return nil, fmt.Errorf("action %q: %w", name, err)
		}
		h.actions[name] = act
	}

	// Add action-related template functions (must be done after h is initialized)
	h.funcMap["itemActions"] = func(item scanner.Item) []map[string]string {
		names := h.ApplicableActions(item.Frontmatter)
		result := make([]map[string]string, 0, len(names))
		for _, name := range names {
			if act, ok := h.actions[name]; ok {
				result = append(result, map[string]string{
					"Name":        name,
					"Description": act.Description,
				})
			}
		}
		return result
	}
	h.funcMap["actionURL"] = func(viewID, itemID, actionName string) string {
		return fmt.Sprintf("/views/%s/items/%s/actions/%s", viewID, itemID, actionName)
	}

	h.funcMap["markdown"] = func(s string) template.HTML {
		s = wikiLinkRegex.ReplaceAllStringFunc(s, func(match string) string {
			submatches := wikiLinkRegex.FindStringSubmatch(match)
			if len(submatches) < 2 {
				return match
			}
			idVal := strings.ToLower(submatches[1])
			item, err := h.Store.GetByID(idVal)
			if err != nil {
				return match
			}

			viewID := item.Type + "s"
			if _, ok := h.Cfg.Views[viewID]; !ok {
				for k := range h.Cfg.Views {
					if strings.Contains(k, item.Type) {
						viewID = k
						break
					}
				}
			}
			if _, ok := h.Cfg.Views[viewID]; !ok {
				if h.Cfg.DefaultView != "" {
					viewID = h.Cfg.DefaultView
				} else {
					viewID = "notes"
				}
			}

			viewCfg := h.Cfg.Views[viewID]
			title := item.Title(viewCfg.TitleField)
			if title == "" {
				title = submatches[1]
			}
			return fmt.Sprintf("[%s](/views/%s/%s)", title, viewID, idVal)
		})

		var buf bytes.Buffer
		if err := goldmark.Convert([]byte(s), &buf); err != nil {
			return template.HTML(template.HTMLEscapeString(s))
		}
		sanitized := bluemonday.UGCPolicy().SanitizeBytes(buf.Bytes())
		return template.HTML(sanitized)
	}

	// Pre-parsing templates layout
	layoutPath := "templates/layout.html"
	err := fs.WalkDir(templateFS, "templates", func(path string, d fs.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		if path == layoutPath || !strings.HasSuffix(path, ".html") {
			return nil
		}
		// Key is the relative path after "templates/"
		name := strings.TrimPrefix(path, "templates/")
		tmpl, parseErr := template.New("").Funcs(h.funcMap).ParseFS(templateFS, layoutPath, path)
		if parseErr != nil {
			return fmt.Errorf("parsing template %q: %w", name, parseErr)
		}
		h.templates[name] = tmpl
		return nil
	})
	if err != nil {
		return nil, fmt.Errorf("pre-parsing templates: %w", err)
	}

	h.configFiles = make(map[string]time.Time)
	h.updateConfigFilesList()

	return h, nil
}

func NewWithStore(cfg *config.Config, baseDir string, store ItemStore, templateFS fs.FS) (*Handlers, error) {
	h, err := New(cfg, baseDir, nil, nil, templateFS)
	if err != nil {
		return nil, err
	}
	h.Store = store
	return h, nil
}

func NewSyncServer(cfg *config.Config, baseDir string, s *syncsvc.Server, templateFS fs.FS) (*Handlers, error) {
	h, err := NewWithStore(cfg, baseDir, s, templateFS)
	if err != nil {
		return nil, err
	}
	h.SyncServer = s
	s.SetOnConfigChanged(func() {
		h.MarkConfigChanged()
	})
	return h, nil
}

// ApplicableActions returns the names of actions that match the given item's frontmatter.
func (h *Handlers) ApplicableActions(fm map[string]interface{}) []string {
	var names []string
	for name, act := range h.actions {
		if act.Match(fm) {
			names = append(names, name)
		}
	}
	return names
}

// scanAndFilter reads all items from cache and returns those matching the given view.
func (h *Handlers) scanAndFilter(viewID string) ([]scanner.Item, error) {
	items, err := h.Store.All()
	if err != nil {
		return nil, err
	}

	prog := h.filters[viewID]
	if prog == nil {
		return nil, fmt.Errorf("no filter for view %q", viewID)
	}

	var result []scanner.Item
	for _, item := range items {
		ok, _ := prog.Eval(item.Frontmatter)
		if ok {
			result = append(result, item)
		}
	}
	return result, nil
}

// findItem looks up an item by ID (frontmatter "id" field) or filename fallback.
func (h *Handlers) findItem(items []scanner.Item, itemID string) (scanner.Item, bool) {
	// Try by frontmatter id first
	for _, item := range items {
		if id, ok := item.Frontmatter["id"].(string); ok && id == itemID {
			return item, true
		}
	}
	// Filename fallback: strip extension from basename
	for _, item := range items {
		base := item.Path[strings.LastIndex(item.Path, "/")+1:]
		name := strings.TrimSuffix(base, ".md")
		if name == itemID {
			return item, true
		}
	}
	return scanner.Item{}, false
}

// TimingMiddleware records the request start time in context.
func (h *Handlers) TimingMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		ctx := context.WithValue(r.Context(), requestStartKey, time.Now())
		next.ServeHTTP(w, r.WithContext(ctx))
	})
}

// CSRFMiddleware protects unsafe POST requests from Cross-Site Request Forgery.
func (h *Handlers) CSRFMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodPost || r.Method == http.MethodPut || r.Method == http.MethodPatch || r.Method == http.MethodDelete {
			// Skip webhook requests which are designed to be cross-origin
			if strings.HasPrefix(r.URL.Path, "/webhook/") {
				next.ServeHTTP(w, r)
				return
			}

			// Validate
			if !h.validateCSRF(r) {
				http.Error(w, "Forbidden - CSRF validation failed", http.StatusForbidden)
				return
			}
		}
		next.ServeHTTP(w, r)
	})
}

func (h *Handlers) validateCSRF(r *http.Request) bool {
	// Sec-Fetch-Site: cross-site is always rejected
	if r.Header.Get("Sec-Fetch-Site") == "cross-site" {
		return false
	}

	// Helper to check if a host is local
	isLocal := func(host string) bool {
		// Strip port if present
		if sh, _, err := net.SplitHostPort(host); err == nil {
			host = sh
		}
		return host == "localhost" || host == "127.0.0.1" || host == "::1"
	}

	// Validate origin
	if origin := r.Header.Get("Origin"); origin != "" {
		u, err := url.Parse(origin)
		if err != nil {
			return false
		}
		if u.Host != r.Host && (!isLocal(u.Host) || !isLocal(r.Host)) {
			return false
		}
	}

	// Validate referer
	if referer := r.Header.Get("Referer"); referer != "" {
		u, err := url.Parse(referer)
		if err != nil {
			return false
		}
		if u.Host != r.Host && (!isLocal(u.Host) || !isLocal(r.Host)) {
			return false
		}
	}

	return true
}

func (h *Handlers) render(w http.ResponseWriter, r *http.Request, name string, data map[string]interface{}) {
	// Inject timing info
	requestStart, _ := data["_requestStart"].(time.Time)
	parseTime, _ := data["_parseTime"].(time.Duration)

	totalTime := time.Since(requestStart)
	data["FooterTotalTime"] = fmt.Sprintf("%.2fms", float64(totalTime.Microseconds())/1000)
	data["FooterParseTime"] = fmt.Sprintf("%.2fms", float64(parseTime.Microseconds())/1000)
	data["FooterHostname"] = h.hostname
	data["FooterVersion"] = version.String()

	// Inject nav data for layout
	data["NavViews"] = h.Cfg.OrderedViews()
	data["Config"] = h.Cfg
	data["SyncServerEnabled"] = h.SyncServer != nil

	tmpl, ok := h.templates[name]
	if !ok {
		http.Error(w, "Template not found: "+name, 500)
		return
	}

	w.Header().Set("Content-Type", "text/html; charset=utf-8")

	// For htmx requests, render only the partial (main + footer OOB swap).
	// This skips the full HTML document, head, nav — saving ~2KB transfer.
	templateName := "layout"
	if r.Header.Get("HX-Request") == "true" {
		templateName = "partial"
	}

	if err := tmpl.ExecuteTemplate(w, templateName, data); err != nil {
		http.Error(w, err.Error(), 500)
	}
}

// newData creates a data map with request timing context pre-filled.
func newData(r *http.Request) map[string]interface{} {
	start, _ := r.Context().Value(requestStartKey).(time.Time)
	return map[string]interface{}{
		"_requestStart": start,
	}
}

func (h *Handlers) MarkConfigChanged() {
	h.configChangedMu.Lock()
	h.configChanged = true
	h.configChangedMu.Unlock()
}

func (h *Handlers) isConfigChanged() bool {
	h.configChangedMu.Lock()
	defer h.configChangedMu.Unlock()
	return h.configChanged
}

func (h *Handlers) updateConfigFilesList() {
	h.configChangedMu.Lock()
	defer h.configChangedMu.Unlock()
	files := make(map[string]time.Time)
	entries, err := os.ReadDir(h.BaseDir)
	if err == nil {
		for _, entry := range entries {
			if entry.IsDir() || filepath.Ext(entry.Name()) != ".toml" || entry.Name() == ".exo.toml" {
				continue
			}
			path := filepath.Join(h.BaseDir, entry.Name())
			if info, err := os.Stat(path); err == nil {
				files[path] = info.ModTime()
			}
		}
	}
	exoDir := filepath.Join(h.BaseDir, ".exo")
	if info, err := os.Stat(exoDir); err == nil && info.IsDir() {
		entries, err := os.ReadDir(exoDir)
		if err == nil && len(files) == 0 {
			for _, entry := range entries {
				if !entry.IsDir() && filepath.Ext(entry.Name()) == ".toml" && entry.Name() != "tui.toml" && entry.Name() != "serve.toml" {
					path := filepath.Join(exoDir, entry.Name())
					if info, err := os.Stat(path); err == nil {
						files[path] = info.ModTime()
					}
				}
			}
		}
	}
	if len(files) == 0 {
		path := filepath.Join(h.BaseDir, ".exo.toml")
		if info, err := os.Stat(path); err == nil {
			files[path] = info.ModTime()
		}
	}
	h.configFiles = files
}

func (h *Handlers) detectConfigChanges() bool {
	h.configChangedMu.Lock()
	defer h.configChangedMu.Unlock()
	files := make(map[string]time.Time)
	entries, err := os.ReadDir(h.BaseDir)
	if err == nil {
		for _, entry := range entries {
			if entry.IsDir() || filepath.Ext(entry.Name()) != ".toml" || entry.Name() == ".exo.toml" {
				continue
			}
			path := filepath.Join(h.BaseDir, entry.Name())
			if info, err := os.Stat(path); err == nil {
				files[path] = info.ModTime()
			}
		}
	}
	exoDir := filepath.Join(h.BaseDir, ".exo")
	if info, err := os.Stat(exoDir); err == nil && info.IsDir() {
		entries, err := os.ReadDir(exoDir)
		if err == nil && len(files) == 0 {
			for _, entry := range entries {
				if !entry.IsDir() && filepath.Ext(entry.Name()) == ".toml" && entry.Name() != "tui.toml" && entry.Name() != "serve.toml" {
					path := filepath.Join(exoDir, entry.Name())
					if info, err := os.Stat(path); err == nil {
						files[path] = info.ModTime()
					}
				}
			}
		}
	}
	if len(files) == 0 {
		path := filepath.Join(h.BaseDir, ".exo.toml")
		if info, err := os.Stat(path); err == nil {
			files[path] = info.ModTime()
		}
	}

	if len(files) != len(h.configFiles) {
		return true
	}
	for path, modTime := range files {
		oldTime, ok := h.configFiles[path]
		if !ok || !modTime.Equal(oldTime) {
			return true
		}
	}
	return false
}

func (h *Handlers) ensureConfigUpdated() {
	var changed bool
	if h.SyncServer != nil {
		changed = h.isConfigChanged()
	} else {
		h.configChangedMu.Lock()
		now := time.Now()
		if now.Sub(h.lastConfigCheck) > 1*time.Second {
			h.lastConfigCheck = now
			h.configChangedMu.Unlock()
			changed = h.detectConfigChanges()
		} else {
			h.configChangedMu.Unlock()
		}
	}

	if changed {
		if err := h.reloadConfig(); err != nil {
			fmt.Fprintf(os.Stderr, "handlers: failed to reload config: %v\n", err)
		}
	}
}

func (h *Handlers) reloadConfig() error {
	h.cfgMu.Lock()
	defer h.cfgMu.Unlock()

	var cfg *config.Config
	var err error
	if h.SyncServer != nil {
		cfg, err = h.SyncServer.LoadConfig()
	} else {
		cfg, err = config.Load(h.BaseDir)
	}
	if err != nil {
		return err
	}

	filters := make(map[string]*filter.Program)
	subviewFilters := make(map[string]*filter.Program)
	for id, vc := range cfg.Views {
		prog, err := filter.Compile(vc.Filter)
		if err != nil {
			return fmt.Errorf("view %q: compiling filter: %w", id, err)
		}
		filters[id] = prog

		for _, sv := range vc.Subviews {
			if sv.Filter == "" || sv.Filter == "true" {
				continue
			}
			svProg, err := filter.Compile(sv.Filter)
			if err != nil {
				return fmt.Errorf("view %q subview %q: compiling filter: %w", id, sv.Name, err)
			}
			subviewFilters[id+"\x00"+sv.Name] = svProg
		}
	}

	actions := make(map[string]*action.Action)
	for name, ac := range cfg.Actions {
		act, err := action.Compile(name, ac)
		if err != nil {
			return fmt.Errorf("action %q: %w", name, err)
		}
		actions[name] = act
	}

	h.Cfg = cfg
	h.filters = filters
	h.subviewFilters = subviewFilters
	h.actions = actions

	if h.SyncServer == nil {
		h.updateConfigFilesList()
	} else {
		h.configChangedMu.Lock()
		h.configChanged = false
		h.configChangedMu.Unlock()
	}

	return nil
}

func (h *Handlers) ConfigReloadMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h.ensureConfigUpdated()

		h.cfgMu.RLock()
		defer h.cfgMu.RUnlock()

		next.ServeHTTP(w, r)
	})
}
