package config

import (
	"context"
	_ "embed"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path"
	"path/filepath"
	"strings"
	"time"

	"github.com/yuin/gopher-lua"
)

// fennelSource is Fennel 1.6.1, downloaded from fennel-lang.org. See the
// SPDX/provenance header in fennel/fennel.lua.
//
//go:embed fennel/fennel.lua
var fennelSource string

type callable struct {
	fn     *lua.LFunction
	source string
}
type runtime struct {
	L        *lua.LState
	compiler lua.LValue
	modules  map[string]string
	loaded   map[string]lua.LValue
	loading  []string
	baseDir  string
}
type Note struct {
	ID, Path, Type, Body string
	Tags                 []string
	Frontmatter          map[string]interface{}
}

const maxCapabilityFileBytes = 1 << 20

func newRuntime(contents []NamedContent) (*runtime, error) {
	r := &runtime{modules: map[string]string{}, loaded: map[string]lua.LValue{}}
	for _, c := range contents {
		name := strings.TrimPrefix(path.Clean(strings.ReplaceAll(c.Name, "\\", "/")), "./")
		if name == "." || strings.HasPrefix(name, "../") || path.IsAbs(name) {
			return nil, fmt.Errorf("invalid configuration path %q", c.Name)
		}
		if name != "exo.fnl" && (!strings.HasPrefix(name, "modules/") || (path.Ext(name) != ".fnl" && path.Ext(name) != ".lua")) {
			return nil, fmt.Errorf("invalid configuration path %q", c.Name)
		}
		if _, exists := r.modules[name]; exists {
			return nil, fmt.Errorf("duplicate configuration path %q", name)
		}
		r.modules[name] = string(c.Content)
	}
	if _, ok := r.modules["exo.fnl"]; !ok {
		return nil, fmt.Errorf("workspace configuration must contain exo.fnl")
	}
	r.L = lua.NewState()
	// The vendored compiler is trusted; its own package preloads are required to
	// bootstrap. Workspace execution below receives a stripped environment.
	compilerSource := strings.TrimPrefix(fennelSource, "#!/usr/bin/env lua\n")
	if cli := strings.Index(compilerSource, "local help = \"Usage:"); cli >= 0 {
		compilerSource = compilerSource[:cli]
	}
	r.L.SetGlobal("arg", r.L.NewTable())
	if err := r.L.DoString(compilerSource + "\n_G.__exo_fennel = require('fennel')"); err != nil {
		r.L.Close()
		return nil, fmt.Errorf("initializing Fennel compiler: %w", err)
	}
	r.compiler = r.L.GetGlobal("__exo_fennel")
	r.L.SetGlobal("__exo_fennel", lua.LNil)
	r.L.SetGlobal("require", r.L.NewFunction(r.require))
	r.L.SetGlobal("assoc", r.L.NewFunction(luaAssoc))
	// Fennel accepts hyphenated names while Lua identifiers use underscores.
	// Export both forms so workspace code can use its native spelling.
	for _, name := range []string{"has-tag", "has_tag", "__fnl_global__has_2dtag"} {
		r.L.SetGlobal(name, r.L.NewFunction(luaHasTag))
	}
	for _, name := range []string{"add-tag", "add_tag", "__fnl_global__add_2dtag"} {
		r.L.SetGlobal(name, r.L.NewFunction(luaAddTag))
	}
	for _, name := range []string{"remove-tag", "remove_tag", "__fnl_global__remove_2dtag"} {
		r.L.SetGlobal(name, r.L.NewFunction(luaRemoveTag))
	}
	r.L.SetGlobal("now", r.L.NewFunction(luaNow))
	r.L.SetGlobal("dofile", lua.LNil)
	r.L.SetGlobal("loadfile", lua.LNil)
	r.L.SetGlobal("io", lua.LNil)
	r.L.SetGlobal("os", lua.LNil)
	r.L.SetGlobal("debug", lua.LNil)
	r.L.SetGlobal("package", lua.LNil)
	return r, nil
}

func (r *runtime) compile(source, filename string) (string, error) {
	fennel, ok := r.compiler.(*lua.LTable)
	if !ok {
		return "", fmt.Errorf("Fennel compiler unavailable")
	}
	compile := fennel.RawGetString("compileString")
	if compile == lua.LNil {
		compile = fennel.RawGetString("compile-string")
	}
	if err := r.L.CallByParam(lua.P{Fn: compile, NRet: 1, Protect: true}, lua.LString(source), r.L.NewTable()); err != nil {
		return "", fmt.Errorf("compiling %s: %w", filename, err)
	}
	result := r.L.Get(-1)
	r.L.Pop(1)
	if s, ok := result.(lua.LString); ok {
		return string(s), nil
	}
	return "", fmt.Errorf("compiling %s: compiler returned %s", filename, result.Type())
}

func (r *runtime) execute(source, filename string) (lua.LValue, error) {
	code := source
	if path.Ext(filename) == ".fnl" {
		var err error
		code, err = r.compile(source, filename)
		if err != nil {
			return nil, err
		}
	}
	fn, err := r.L.LoadString(code)
	if err != nil {
		return nil, fmt.Errorf("loading %s: %w", filename, err)
	}
	if err := r.L.CallByParam(lua.P{Fn: fn, NRet: 1, Protect: true}); err != nil {
		return nil, fmt.Errorf("executing %s: %w", filename, err)
	}
	v := r.L.Get(-1)
	r.L.Pop(1)
	return v, nil
}

func (r *runtime) require(L *lua.LState) int {
	name := L.CheckString(1)
	name = strings.ReplaceAll(name, ".", "/")
	if name == "" || strings.HasPrefix(name, "/") || strings.Contains(name, "..") || !strings.HasPrefix(name, "modules/") {
		L.RaiseError("module %q is outside workspace modules", name)
		return 0
	}
	for _, active := range r.loading {
		if active == name {
			L.RaiseError("module cycle: %s -> %s", strings.Join(r.loading, " -> "), name)
			return 0
		}
	}
	if value, ok := r.loaded[name]; ok {
		L.Push(value)
		return 1
	}
	for _, ext := range []string{".fnl", ".lua"} {
		file := name + ext
		source, ok := r.modules[file]
		if !ok {
			continue
		}
		r.loading = append(r.loading, name)
		value, err := r.execute(source, file)
		r.loading = r.loading[:len(r.loading)-1]
		if err != nil {
			L.RaiseError("%v", err)
			return 0
		}
		r.loaded[name] = value
		L.Push(value)
		return 1
	}
	L.RaiseError("workspace module %q not found", name)
	return 0
}

func luaAssoc(L *lua.LState) int {
	t := L.CheckTable(1)
	key := L.CheckAny(2)
	value := L.CheckAny(3)
	copy := L.NewTable()
	t.ForEach(func(k, v lua.LValue) { copy.RawSet(k, v) })
	copy.RawSet(key, value)
	L.Push(copy)
	return 1
}

func luaHasTag(L *lua.LState) int {
	tags := L.CheckTable(1)
	tag := L.CheckString(2)
	found := false
	tags.ForEach(func(_ lua.LValue, value lua.LValue) {
		if luaString(value) == tag {
			found = true
		}
	})
	L.Push(lua.LBool(found))
	return 1
}

func luaAddTag(L *lua.LState) int {
	tags := L.CheckTable(1)
	tag := L.CheckString(2)
	copy := L.NewTable()
	found := false
	tags.ForEach(func(_ lua.LValue, value lua.LValue) {
		copy.Append(value)
		if luaString(value) == tag {
			found = true
		}
	})
	if !found {
		copy.Append(lua.LString(tag))
	}
	L.Push(copy)
	return 1
}

func luaRemoveTag(L *lua.LState) int {
	tags := L.CheckTable(1)
	tag := L.CheckString(2)
	copy := L.NewTable()
	tags.ForEach(func(_ lua.LValue, value lua.LValue) {
		if luaString(value) != tag {
			copy.Append(value)
		}
	})
	L.Push(copy)
	return 1
}

func luaNow(L *lua.LState) int {
	L.Push(lua.LString(time.Now().UTC().Format(time.RFC3339)))
	return 1
}

func loadWorkspace(contents []NamedContent) (*Config, error) {
	r, err := newRuntime(contents)
	if err != nil {
		return nil, err
	}
	v, err := r.execute(r.modules["exo.fnl"], "exo.fnl")
	if err != nil {
		r.L.Close()
		return nil, err
	}
	t, ok := v.(*lua.LTable)
	if !ok {
		r.L.Close()
		return nil, fmt.Errorf("exo.fnl must return a table")
	}
	cfg := &Config{Views: map[string]ViewConfig{}, Actions: map[string]ActionConfig{}, runtime: r}
	cfg.DefaultView = luaString(t.RawGetString("default-view"))
	if err := decodeViews(r, cfg, t.RawGetString("views")); err != nil {
		r.L.Close()
		return nil, err
	}
	if err := decodeActions(cfg, t.RawGetString("actions")); err != nil {
		r.L.Close()
		return nil, err
	}
	if err := cfg.validate(); err != nil {
		r.L.Close()
		return nil, fmt.Errorf("invalid configuration: %w", err)
	}
	cfg.addBuiltInViews()
	return cfg, nil
}

func luaString(v lua.LValue) string {
	if s, ok := v.(lua.LString); ok {
		return string(s)
	}
	return ""
}
func luaBool(v lua.LValue) bool { b, _ := v.(lua.LBool); return bool(b) }
func callableOf(v lua.LValue, source string) callable {
	f, _ := v.(*lua.LFunction)
	return callable{fn: f, source: source}
}
func decodeViews(r *runtime, cfg *Config, value lua.LValue) error {
	t, ok := value.(*lua.LTable)
	if !ok {
		return fmt.Errorf(":views must be a table")
	}
	var err error
	t.ForEach(func(k, value lua.LValue) {
		if err != nil {
			return
		}
		id := luaString(k)
		vt, ok := value.(*lua.LTable)
		if !ok {
			err = fmt.Errorf("view %q must be a table", id)
			return
		}
		v := ViewConfig{Name: luaString(vt.RawGetString("name")), Key: luaString(vt.RawGetString("key")), Filter: "true", ShowTags: luaBool(vt.RawGetString("show-tags")), TitleField: luaString(vt.RawGetString("title-field")), SubtitleField: luaString(vt.RawGetString("subtitle-field")), SortField: luaString(vt.RawGetString("sort-field")), SortOrder: luaString(vt.RawGetString("sort-order")), PreviewTemplate: luaString(vt.RawGetString("preview-template")), StatsTemplate: luaString(vt.RawGetString("stats-template")), when: callableOf(vt.RawGetString("when"), "view "+id)}
		svs, _ := vt.RawGetString("subviews").(*lua.LTable)
		if svs != nil {
			svs.ForEach(func(_ lua.LValue, svv lua.LValue) {
				st, ok := svv.(*lua.LTable)
				if !ok {
					err = fmt.Errorf("view %q subview must be a table", id)
					return
				}
				v.Subviews = append(v.Subviews, SubviewConfig{Name: luaString(st.RawGetString("name")), Filter: "true", when: callableOf(st.RawGetString("when"), "view "+id)})
			})
		}
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
			v.Subviews = []SubviewConfig{{Name: "All", Filter: "true", when: alwaysCallable(r)}}
		}
		cfg.Views[id] = v
	})
	return err
}
func alwaysCallable(r *runtime) callable {
	v, _ := r.execute("return function(_) return true end", "builtin.lua")
	return callableOf(v, "builtin")
}
func decodeActions(cfg *Config, value lua.LValue) error {
	if value == lua.LNil {
		return nil
	}
	t, ok := value.(*lua.LTable)
	if !ok {
		return fmt.Errorf(":actions must be a table")
	}
	var err error
	t.ForEach(func(k, v lua.LValue) {
		if err != nil {
			return
		}
		name := luaString(k)
		at, ok := v.(*lua.LTable)
		if !ok {
			err = fmt.Errorf("action %q must be a table", name)
			return
		}
		a := ActionConfig{Description: luaString(at.RawGetString("description")), when: callableOf(at.RawGetString("when"), "action "+name), run: callableOf(at.RawGetString("run"), "action "+name), runtime: cfg.runtime, name: name}
		if ps, ok := at.RawGetString("permissions").(*lua.LTable); ok {
			ps.ForEach(func(_ lua.LValue, p lua.LValue) { a.Permissions = append(a.Permissions, luaString(p)) })
		}
		cfg.Actions[name] = a
	})
	return err
}

func (r *runtime) callPredicate(fn callable, note Note) (bool, error) {
	if fn.fn == nil {
		return false, fmt.Errorf("%s: :when must be a function", fn.source)
	}
	// Capability helpers are deny-by-default until a matching local grant is
	// loaded. Always pass an empty capability object so two-argument actions are
	// portable across hosts.
	v, err := r.call(fn.fn, noteTable(r.L, note), r.L.NewTable())
	if err != nil {
		return false, fmt.Errorf("%s: %w", fn.source, err)
	}
	b, ok := v.(lua.LBool)
	if !ok {
		return false, fmt.Errorf("%s: predicate must return a boolean", fn.source)
	}
	return bool(b), nil
}
func (r *runtime) callAction(fn callable, note Note, exo *lua.LTable) (Note, error) {
	if fn.fn == nil {
		return Note{}, fmt.Errorf("%s: :run must be a function", fn.source)
	}
	v, err := r.call(fn.fn, noteTable(r.L, note), exo)
	if err != nil {
		return Note{}, fmt.Errorf("%s: %w", fn.source, err)
	}
	t, ok := v.(*lua.LTable)
	if !ok {
		return Note{}, fmt.Errorf("%s: action must return a note table", fn.source)
	}
	return noteFromTable(t)
}
func (r *runtime) call(fn *lua.LFunction, args ...lua.LValue) (lua.LValue, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()
	r.L.SetContext(ctx)
	defer r.L.RemoveContext()
	if err := r.L.CallByParam(lua.P{Fn: fn, NRet: 1, Protect: true}, args...); err != nil {
		return nil, err
	}
	v := r.L.Get(-1)
	r.L.Pop(1)
	return v, nil
}

func (r *runtime) capabilities(g PermissionGrant) *lua.LTable {
	exo := r.L.NewTable()
	fs := r.L.NewTable()
	if len(g.Read) > 0 {
		fs.RawSetString("read", r.L.NewFunction(func(L *lua.LState) int {
			name := L.CheckString(1)
			if filepath.IsAbs(name) || strings.Contains(filepath.ToSlash(name), "..") {
				L.RaiseError("filesystem read denied: %s", name)
			}
			allowed := false
			for _, pattern := range g.Read {
				if ok, _ := path.Match(pattern, filepath.ToSlash(name)); ok {
					allowed = true
					break
				}
			}
			if !allowed {
				L.RaiseError("filesystem read denied: %s", name)
			}
			data, err := os.ReadFile(filepath.Join(r.baseDir, name))
			if err != nil {
				L.RaiseError("%v", err)
			}
			if len(data) > maxCapabilityFileBytes {
				L.RaiseError("filesystem read exceeds %d byte limit", maxCapabilityFileBytes)
			}
			L.Push(lua.LString(data))
			return 1
		}))
	}
	if len(g.Write) > 0 {
		fs.RawSetString("write", r.L.NewFunction(func(L *lua.LState) int {
			name, content := L.CheckString(1), L.CheckString(2)
			if !matchesGrant(name, g.Write) {
				L.RaiseError("filesystem write denied: %s", name)
			}
			if len(content) > maxCapabilityFileBytes {
				L.RaiseError("filesystem write exceeds %d byte limit", maxCapabilityFileBytes)
			}
			full := filepath.Join(r.baseDir, name)
			if err := os.MkdirAll(filepath.Dir(full), 0755); err != nil {
				L.RaiseError("%v", err)
			}
			if err := os.WriteFile(full, []byte(content), 0644); err != nil {
				L.RaiseError("%v", err)
			}
			return 0
		}))
	}
	exo.RawSetString("filesystem", fs)
	if len(g.Origins) > 0 {
		network := r.L.NewTable()
		network.RawSetString("get", r.L.NewFunction(func(L *lua.LState) int {
			raw := L.CheckString(1)
			u, err := url.Parse(raw)
			if err != nil || u.Scheme != "https" || !originAllowed(u, g.Origins) {
				L.RaiseError("network request denied: %s", raw)
			}
			client := &http.Client{Timeout: 10 * time.Second, CheckRedirect: func(req *http.Request, via []*http.Request) error {
				if !originAllowed(req.URL, g.Origins) {
					return http.ErrUseLastResponse
				}
				return nil
			}}
			resp, err := client.Get(raw)
			if err != nil {
				L.RaiseError("%v", err)
			}
			defer resp.Body.Close()
			body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
			if err != nil {
				L.RaiseError("%v", err)
			}
			L.Push(lua.LString(body))
			return 1
		}))
		exo.RawSetString("network", network)
	}
	return exo
}

func matchesGrant(name string, patterns []string) bool {
	if filepath.IsAbs(name) || strings.Contains(filepath.ToSlash(name), "..") {
		return false
	}
	for _, pattern := range patterns {
		if ok, _ := path.Match(pattern, filepath.ToSlash(name)); ok {
			return true
		}
	}
	return false
}
func originAllowed(u *url.URL, origins []string) bool {
	for _, origin := range origins {
		if origin == u.Scheme+"://"+u.Host {
			return true
		}
	}
	return false
}

func applyPermissions(cfg *Config, value lua.LValue) error {
	if containsFunction(value) {
		return fmt.Errorf("permissions.fnl must contain data only")
	}
	root, ok := value.(*lua.LTable)
	if !ok {
		return fmt.Errorf("permissions.fnl must return a table")
	}
	actions, ok := root.RawGetString("actions").(*lua.LTable)
	if !ok {
		return fmt.Errorf("permissions.fnl requires :actions table")
	}
	var result error
	actions.ForEach(func(k, v lua.LValue) {
		if result != nil {
			return
		}
		name := luaString(k)
		action, ok := cfg.Actions[name]
		if !ok {
			result = fmt.Errorf("permissions for unknown action %q", name)
			return
		}
		grant, ok := v.(*lua.LTable)
		if !ok {
			result = fmt.Errorf("permissions for %q must be a table", name)
			return
		}
		fs, _ := grant.RawGetString("filesystem").(*lua.LTable)
		if fs != nil && declared(action.Permissions, "filesystem") {
			if reads, ok := fs.RawGetString("read").(*lua.LTable); ok {
				reads.ForEach(func(_ lua.LValue, v lua.LValue) { action.grant.Read = append(action.grant.Read, luaString(v)) })
			}
			if writes, ok := fs.RawGetString("write").(*lua.LTable); ok {
				writes.ForEach(func(_ lua.LValue, v lua.LValue) { action.grant.Write = append(action.grant.Write, luaString(v)) })
			}
		}
		if network, ok := grant.RawGetString("network").(*lua.LTable); ok && declared(action.Permissions, "network") {
			if origins, ok := network.RawGetString("origins").(*lua.LTable); ok {
				origins.ForEach(func(_ lua.LValue, v lua.LValue) { action.grant.Origins = append(action.grant.Origins, luaString(v)) })
			}
		}
		cfg.Actions[name] = action
	})
	return result
}

func containsFunction(value lua.LValue) bool {
	if value.Type() == lua.LTFunction {
		return true
	}
	table, ok := value.(*lua.LTable)
	if !ok {
		return false
	}
	found := false
	table.ForEach(func(_, value lua.LValue) {
		if containsFunction(value) {
			found = true
		}
	})
	return found
}
func declared(items []string, name string) bool {
	for _, item := range items {
		if item == name {
			return true
		}
	}
	return false
}

func noteTable(L *lua.LState, note Note) *lua.LTable {
	t := L.NewTable()
	t.RawSetString("id", lua.LString(note.ID))
	t.RawSetString("path", lua.LString(note.Path))
	t.RawSetString("type", lua.LString(note.Type))
	t.RawSetString("body", lua.LString(note.Body))
	tags := L.NewTable()
	for _, tag := range note.Tags {
		tags.Append(lua.LString(tag))
	}
	t.RawSetString("tags", tags)
	t.RawSetString("frontmatter", goTable(L, note.Frontmatter))
	return t
}
func goTable(L *lua.LState, m map[string]interface{}) *lua.LTable {
	t := L.NewTable()
	for k, v := range m {
		t.RawSetString(k, goValue(L, v))
	}
	return t
}
func goValue(L *lua.LState, v interface{}) lua.LValue {
	switch x := v.(type) {
	case nil:
		return lua.LNil
	case string:
		return lua.LString(x)
	case bool:
		return lua.LBool(x)
	case int:
		return lua.LNumber(x)
	case int64:
		return lua.LNumber(x)
	case float64:
		return lua.LNumber(x)
	case []string:
		t := L.NewTable()
		for _, s := range x {
			t.Append(lua.LString(s))
		}
		return t
	case []interface{}:
		t := L.NewTable()
		for _, e := range x {
			t.Append(goValue(L, e))
		}
		return t
	case map[string]interface{}:
		return goTable(L, x)
	default:
		return lua.LString(fmt.Sprint(v))
	}
}
func noteFromTable(t *lua.LTable) (Note, error) {
	n := Note{ID: luaString(t.RawGetString("id")), Path: luaString(t.RawGetString("path")), Type: luaString(t.RawGetString("type")), Body: luaString(t.RawGetString("body"))}
	if n.Path == "" || n.Type == "" {
		return Note{}, fmt.Errorf("action result requires :path and :type")
	}
	tags, ok := t.RawGetString("tags").(*lua.LTable)
	if !ok {
		return Note{}, fmt.Errorf("action result requires :tags table")
	}
	tags.ForEach(func(_ lua.LValue, v lua.LValue) { n.Tags = append(n.Tags, luaString(v)) })
	fm, ok := t.RawGetString("frontmatter").(*lua.LTable)
	if !ok {
		return Note{}, fmt.Errorf("action result requires :frontmatter table")
	}
	n.Frontmatter = tableMap(fm)
	return n, nil
}
func tableMap(t *lua.LTable) map[string]interface{} {
	m := map[string]interface{}{}
	t.ForEach(func(k, v lua.LValue) { m[luaString(k)] = goFromLua(v) })
	return m
}
func goFromLua(v lua.LValue) interface{} {
	switch x := v.(type) {
	case lua.LString:
		return string(x)
	case lua.LBool:
		return bool(x)
	case lua.LNumber:
		return float64(x)
	case *lua.LTable:
		isArray := true
		max := 0
		x.ForEach(func(k, v lua.LValue) {
			if n, ok := k.(lua.LNumber); ok && int(n) > max {
				max = int(n)
			} else {
				isArray = false
			}
		})
		if isArray {
			a := make([]interface{}, 0, max)
			for i := 1; i <= max; i++ {
				a = append(a, goFromLua(x.RawGetInt(i)))
			}
			return a
		}
		return tableMap(x)
	default:
		return nil
	}
}

func loadAppTable(source []byte, filename string, cfg *AppConfig) error {
	r, err := newRuntime([]NamedContent{{Name: "exo.fnl", Content: []byte("{}")}})
	if err != nil {
		return err
	}
	defer r.L.Close()
	v, err := r.execute(string(source), filename)
	if err != nil {
		return err
	}
	t, ok := v.(*lua.LTable)
	if !ok {
		return fmt.Errorf("%s must return a table", filename)
	}
	if sync, ok := t.RawGetString("sync").(*lua.LTable); ok {
		cfg.Sync.ServerURL = luaString(sync.RawGetString("server-url"))
		cfg.Sync.ClientID = luaString(sync.RawGetString("client-id"))
		cfg.Sync.KeyPath = luaString(sync.RawGetString("key-path"))
		cfg.Sync.Enabled = luaBool(sync.RawGetString("enabled"))
		cfg.Sync.Server = serverConfig(sync.RawGetString("server"))
	}
	cfg.Server = serverConfig(t.RawGetString("server"))
	return nil
}
func serverConfig(v lua.LValue) SyncServerConfig {
	t, ok := v.(*lua.LTable)
	if !ok {
		return SyncServerConfig{}
	}
	return SyncServerConfig{Enabled: luaBool(t.RawGetString("enabled")), DBPath: luaString(t.RawGetString("db-path")), Listen: luaString(t.RawGetString("listen"))}
}
