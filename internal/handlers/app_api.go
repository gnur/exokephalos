package handlers

import (
	"encoding/json"
	"fmt"
	"net/http"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/scanner"
	"github.com/gnur/exokephalos/internal/syncsvc"
	"gopkg.in/yaml.v3"
)

type appItem struct {
	ID          string                 `json:"id"`
	Path        string                 `json:"path"`
	Type        string                 `json:"type"`
	Title       string                 `json:"title"`
	Subtitle    string                 `json:"subtitle"`
	Tags        []string               `json:"tags"`
	Frontmatter map[string]interface{} `json:"frontmatter"`
	Body        string                 `json:"body"`
	Raw         string                 `json:"raw"`
	UpdatedAt   string                 `json:"updated_at,omitempty"`
	Deleted     bool                   `json:"deleted,omitempty"`
}

type appView struct {
	ID       string            `json:"id"`
	Config   config.ViewConfig `json:"config"`
	ItemIDs  []string          `json:"item_ids"`
	Subviews []appSubview      `json:"subviews"`
}

type appSubview struct {
	Name    string   `json:"name"`
	ItemIDs []string `json:"item_ids"`
}

type appAction struct {
	Name        string `json:"name"`
	Description string `json:"description"`
}

type appBootstrapResponse struct {
	DefaultView       string      `json:"default_view"`
	Views             []appView   `json:"views"`
	Actions           []appAction `json:"actions"`
	Items             []appItem   `json:"items"`
	Revision          int64       `json:"revision"`
	SyncServerEnabled bool        `json:"sync_server_enabled"`
}

type appChangeRequest struct {
	Changes []appChange `json:"changes"`
}

type appChange struct {
	ClientMutationID string                 `json:"client_mutation_id"`
	Op               string                 `json:"op"`
	TargetKind       string                 `json:"target_kind"`
	ID               string                 `json:"id"`
	Path             string                 `json:"path"`
	Frontmatter      map[string]interface{} `json:"frontmatter"`
	Body             string                 `json:"body"`
}

type appChangeResponse struct {
	Revision int64                `json:"revision"`
	Accepted []string             `json:"accepted"`
	Rejected []appChangeRejection `json:"rejected"`
}

type appChangeRejection struct {
	ID    string `json:"id"`
	Error string `json:"error"`
}

type appAPIKeyCreateRequest struct {
	AppName   string `json:"app_name"`
	Filter    string `json:"filter"`
	ExpiresAt string `json:"expires_at"`
}

type appAPIKeyCreateResponse struct {
	Key    string      `json:"key"`
	Record interface{} `json:"record"`
}

func (h *Handlers) AppBootstrap(w http.ResponseWriter, r *http.Request) {
	items, err := h.Store.All()
	if err != nil {
		writeAPIError(w, "reading items", http.StatusInternalServerError)
		return
	}
	resp := appBootstrapResponse{
		DefaultView:       h.Cfg.DefaultView,
		Views:             h.appViews(),
		Actions:           h.appActions(),
		Items:             make([]appItem, 0, len(items)),
		Revision:          h.appRevision(),
		SyncServerEnabled: h.SyncServer != nil,
	}
	for _, item := range items {
		resp.Items = append(resp.Items, h.appItem(item))
	}
	sort.Slice(resp.Items, func(i, j int) bool {
		return resp.Items[i].Title < resp.Items[j].Title
	})
	writeAppJSON(w, resp)
}

func (h *Handlers) AppChanges(w http.ResponseWriter, r *http.Request) {
	var req appChangeRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeAPIError(w, "invalid JSON body", http.StatusBadRequest)
		return
	}
	resp := appChangeResponse{
		Revision: h.appRevision(),
		Accepted: []string{},
		Rejected: []appChangeRejection{},
	}
	for _, ch := range req.Changes {
		mutationID := ch.ClientMutationID
		if mutationID == "" {
			mutationID = ch.ID
		}
		rev, err := h.applyAppChange(ch)
		if err != nil {
			resp.Rejected = append(resp.Rejected, appChangeRejection{ID: mutationID, Error: err.Error()})
			continue
		}
		resp.Accepted = append(resp.Accepted, mutationID)
		if rev > resp.Revision {
			resp.Revision = rev
		}
	}
	writeAppJSON(w, resp)
}

func (h *Handlers) AppSyncClients(w http.ResponseWriter, r *http.Request) {
	if h.SyncServer == nil {
		writeAppJSON(w, map[string]interface{}{"clients": []syncsvc.Client{}})
		return
	}
	clients, err := h.SyncServer.Clients()
	if err != nil {
		writeAPIError(w, "reading sync clients", http.StatusInternalServerError)
		return
	}
	if clients == nil {
		clients = []syncsvc.Client{}
	}
	writeAppJSON(w, map[string]interface{}{"clients": clients})
}

func (h *Handlers) AppConfigs(w http.ResponseWriter, r *http.Request) {
	if h.SyncServer == nil {
		writeAPIError(w, "sync server is not enabled", http.StatusNotFound)
		return
	}
	configs, err := h.SyncServer.Configs()
	if err != nil {
		writeAPIError(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeAppJSON(w, map[string]interface{}{"configs": configs})
}

func (h *Handlers) AppConfigUpdate(w http.ResponseWriter, r *http.Request) {
	if h.SyncServer == nil {
		writeAPIError(w, "sync server is not enabled", http.StatusNotFound)
		return
	}
	path := strings.TrimSpace(r.PathValue("path"))
	if !config.IsWorkspacePath(path) {
		writeAPIError(w, "invalid config path", http.StatusBadRequest)
		return
	}
	var req struct {
		Content string `json:"content"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeAPIError(w, "invalid JSON body", http.StatusBadRequest)
		return
	}
	configs, err := h.SyncServer.Configs()
	if err != nil {
		writeAPIError(w, err.Error(), http.StatusInternalServerError)
		return
	}
	contents := make([]config.NamedContent, 0, len(configs)+1)
	found := false
	for _, ch := range configs {
		content := ch.Content
		if ch.Path == path {
			content = req.Content
			found = true
		}
		contents = append(contents, config.NamedContent{Name: ch.Path, Content: []byte(content)})
	}
	if !found {
		contents = append(contents, config.NamedContent{Name: path, Content: []byte(req.Content)})
	}
	sort.Slice(contents, func(i, j int) bool { return contents[i].Name < contents[j].Name })
	if _, err := config.LoadContents(contents); err != nil {
		writeAPIError(w, err.Error(), http.StatusBadRequest)
		return
	}
	if _, err := h.SyncServer.UpsertConfig(path, req.Content); err != nil {
		writeAPIError(w, err.Error(), http.StatusInternalServerError)
		return
	}
	h.MarkConfigChanged()
	writeAppJSON(w, map[string]bool{"ok": true})
}

func (h *Handlers) AppSyncClientApprove(w http.ResponseWriter, r *http.Request) {
	if h.SyncServer == nil {
		writeAPIError(w, "sync server is not enabled", http.StatusNotFound)
		return
	}
	if err := h.SyncServer.ApproveClient(r.PathValue("clientId")); err != nil {
		writeAPIError(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeAppJSON(w, map[string]bool{"ok": true})
}

func (h *Handlers) AppAction(w http.ResponseWriter, r *http.Request) {
	actionName := r.PathValue("actionName")
	act, ok := h.actions[actionName]
	if !ok {
		writeAPIError(w, "action not found", http.StatusNotFound)
		return
	}
	var req struct {
		ItemID string `json:"item_id"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeAPIError(w, "invalid JSON body", http.StatusBadRequest)
		return
	}
	item, err := h.Store.GetByID(req.ItemID)
	if err != nil {
		writeAPIError(w, "item not found", http.StatusNotFound)
		return
	}
	if !act.MatchNote(config.Note{ID: item.ID, Path: item.Path, Type: item.Type, Tags: item.Tags, Frontmatter: item.Frontmatter, Body: item.Body}) {
		writeAPIError(w, "action does not apply to item", http.StatusBadRequest)
		return
	}
	replacement, err := act.Run(config.Note{ID: item.ID, Path: item.Path, Type: item.Type, Tags: item.Tags, Frontmatter: item.Frontmatter, Body: item.Body})
	if err != nil {
		writeAPIError(w, err.Error(), http.StatusBadRequest)
		return
	}
	if err := h.Store.UpdateItem(item.Path, replacement.Frontmatter, replacement.Body); err != nil {
		writeAPIError(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeAppJSON(w, h.appItem(scanner.Item{Path: replacement.Path, Frontmatter: replacement.Frontmatter, Body: replacement.Body, ID: replacement.ID, Type: replacement.Type, Tags: replacement.Tags}))
}

func (h *Handlers) AppItemActions(w http.ResponseWriter, r *http.Request) {
	item, err := h.Store.GetByID(r.PathValue("id"))
	if err != nil {
		writeAPIError(w, "item not found", http.StatusNotFound)
		return
	}
	actions := make([]appAction, 0)
	for name, act := range h.actions {
		if act.MatchNote(config.Note{ID: item.ID, Path: item.Path, Type: item.Type, Tags: item.Tags, Frontmatter: item.Frontmatter, Body: item.Body}) {
			actions = append(actions, appAction{Name: name, Description: act.Description})
		}
	}
	sort.Slice(actions, func(i, j int) bool { return actions[i].Name < actions[j].Name })
	writeAppJSON(w, map[string]interface{}{"actions": actions})
}

func (h *Handlers) AppSyncClientRevoke(w http.ResponseWriter, r *http.Request) {
	if h.SyncServer == nil {
		writeAPIError(w, "sync server is not enabled", http.StatusNotFound)
		return
	}
	if err := h.SyncServer.RevokeClient(r.PathValue("clientId")); err != nil {
		writeAPIError(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeAppJSON(w, map[string]bool{"ok": true})
}

func (h *Handlers) AppPassword(w http.ResponseWriter, r *http.Request) {
	if h.Auth == nil {
		writeAPIError(w, "authentication is not enabled", http.StatusNotFound)
		return
	}
	var req struct {
		CurrentPassword string `json:"current_password"`
		NewPassword     string `json:"new_password"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeAPIError(w, "invalid JSON body", http.StatusBadRequest)
		return
	}
	if strings.TrimSpace(req.NewPassword) == "" {
		writeAPIError(w, "new password is required", http.StatusBadRequest)
		return
	}
	ok, err := h.Auth.VerifyPassword(req.CurrentPassword)
	if err != nil {
		writeAPIError(w, err.Error(), http.StatusInternalServerError)
		return
	}
	if !ok {
		writeAPIError(w, "current password is incorrect", http.StatusForbidden)
		return
	}
	if err := h.Auth.SetPassword(req.NewPassword); err != nil {
		writeAPIError(w, err.Error(), http.StatusInternalServerError)
		return
	}
	cookie, err := h.Auth.LoginCookie(r, true)
	if err != nil {
		writeAPIError(w, err.Error(), http.StatusInternalServerError)
		return
	}
	http.SetCookie(w, cookie)
	writeAppJSON(w, map[string]bool{"ok": true})
}

func (h *Handlers) AppAPIKeys(w http.ResponseWriter, r *http.Request) {
	if h.Auth == nil {
		writeAPIError(w, "authentication is not enabled", http.StatusNotFound)
		return
	}
	keys, err := h.Auth.ListAPIKeys()
	if err != nil {
		writeAPIError(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeAppJSON(w, map[string]interface{}{"keys": keys})
}

func (h *Handlers) AppAPIKeyCreate(w http.ResponseWriter, r *http.Request) {
	if h.Auth == nil {
		writeAPIError(w, "authentication is not enabled", http.StatusNotFound)
		return
	}
	var req appAPIKeyCreateRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeAPIError(w, "invalid JSON body", http.StatusBadRequest)
		return
	}
	expiresAt, err := time.Parse(time.RFC3339, req.ExpiresAt)
	if err != nil {
		if parsed, parseErr := time.Parse("2006-01-02", req.ExpiresAt); parseErr == nil {
			expiresAt = parsed.Add(24*time.Hour - time.Nanosecond)
		} else {
			writeAPIError(w, "invalid expiration date", http.StatusBadRequest)
			return
		}
	}
	raw, record, err := h.Auth.CreateAPIKey(req.AppName, req.Filter, expiresAt)
	if err != nil {
		writeAPIError(w, err.Error(), http.StatusBadRequest)
		return
	}
	writeAppJSON(w, appAPIKeyCreateResponse{Key: raw, Record: record})
}

func (h *Handlers) AppAPIKeyRevoke(w http.ResponseWriter, r *http.Request) {
	if h.Auth == nil {
		writeAPIError(w, "authentication is not enabled", http.StatusNotFound)
		return
	}
	id, err := strconv.ParseInt(r.PathValue("id"), 10, 64)
	if err != nil || id <= 0 {
		writeAPIError(w, "invalid API key id", http.StatusBadRequest)
		return
	}
	if err := h.Auth.RevokeAPIKey(id); err != nil {
		writeAPIError(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeAppJSON(w, map[string]bool{"ok": true})
}

func (h *Handlers) AppEvents(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	flusher, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "streaming unsupported", http.StatusInternalServerError)
		return
	}
	fmt.Fprint(w, ": connected\n\n")
	flusher.Flush()
	ticker := time.NewTicker(5 * time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-r.Context().Done():
			return
		case <-ticker.C:
			fmt.Fprint(w, ": ping\n\n")
			flusher.Flush()
		}
	}
}

func (h *Handlers) appViews() []appView {
	ordered := h.Cfg.OrderedViews()
	views := make([]appView, 0, len(ordered))
	for _, view := range ordered {
		filtered, err := h.scanAndFilter(view.ID)
		itemIDs := make([]string, 0, len(filtered))
		if err == nil {
			for _, item := range filtered {
				if id := markdown.FMString(item.Frontmatter, "id"); id != "" {
					itemIDs = append(itemIDs, id)
				}
			}
		}
		subviews := make([]appSubview, 0, len(view.Config.Subviews))
		for subviewIndex, subview := range view.Config.Subviews {
			subviewIDs := make([]string, 0, len(filtered))
			for _, item := range filtered {
				if ok, err := h.Cfg.MatchSubview(view.ID, subviewIndex, config.Note{ID: item.ID, Path: item.Path, Type: item.Type, Tags: item.Tags, Frontmatter: item.Frontmatter, Body: item.Body}); err == nil && ok {
					if id := markdown.FMString(item.Frontmatter, "id"); id != "" {
						subviewIDs = append(subviewIDs, id)
					}
				}
			}
			subviews = append(subviews, appSubview{Name: subview.Name, ItemIDs: subviewIDs})
		}
		views = append(views, appView{ID: view.ID, Config: view.Config, ItemIDs: itemIDs, Subviews: subviews})
	}
	return views
}

func (h *Handlers) appActions() []appAction {
	actions := make([]appAction, 0, len(h.Cfg.Actions))
	for name, cfg := range h.Cfg.Actions {
		actions = append(actions, appAction{Name: name, Description: cfg.Description})
	}
	sort.Slice(actions, func(i, j int) bool { return actions[i].Name < actions[j].Name })
	return actions
}

func (h *Handlers) appItem(item scanner.Item) appItem {
	id := markdown.FMString(item.Frontmatter, "id")
	if id == "" {
		id = item.ID
	}
	if id == "" {
		id = strings.TrimSuffix(filepath.Base(item.Path), filepath.Ext(item.Path))
	}
	typ := markdown.FMString(item.Frontmatter, "type")
	title := markdown.FMString(item.Frontmatter, "title")
	if title == "" {
		title = id
	}
	subtitle := ""
	if typ != "" {
		viewID := typ + "s"
		if view, ok := h.Cfg.Views[viewID]; ok && view.SubtitleField != "" {
			subtitle = item.Subtitle(view.SubtitleField)
		}
	}
	return appItem{
		ID:          id,
		Path:        h.relItemPath(item.Path),
		Type:        typ,
		Title:       title,
		Subtitle:    subtitle,
		Tags:        item.GetTags(),
		Frontmatter: item.Frontmatter,
		Body:        item.Body,
		Raw:         renderAppRawMarkdown(item.Frontmatter, item.Body),
		UpdatedAt:   time.Now().UTC().Format(time.RFC3339Nano),
	}
}

func (h *Handlers) appRevision() int64 {
	if h.SyncServer != nil {
		return h.SyncServer.LatestRevision()
	}
	return time.Now().UTC().UnixNano()
}

func (h *Handlers) applyAppChange(ch appChange) (int64, error) {
	if ch.TargetKind == "" {
		ch.TargetKind = "item"
	}
	if ch.TargetKind != "item" {
		return 0, fmt.Errorf("unsupported target kind %q", ch.TargetKind)
	}
	if h.SyncServer != nil {
		return h.SyncServer.ApplyChange(syncsvc.Change{
			Op:          ch.Op,
			TargetKind:  ch.TargetKind,
			ID:          ch.ID,
			Path:        ch.Path,
			Frontmatter: ch.Frontmatter,
			Body:        ch.Body,
		})
	}
	switch ch.Op {
	case "delete_item", "delete":
		item, err := h.Store.GetByID(ch.ID)
		if err != nil {
			return 0, err
		}
		return h.appRevision(), h.Store.DeleteItem(item.Path)
	case "upsert_item", "update_item", "create_item", "upsert":
		if ch.ID == "" {
			ch.ID = markdown.FMString(ch.Frontmatter, "id")
		}
		if ch.ID == "" {
			return 0, fmt.Errorf("missing item id")
		}
		if ch.Frontmatter == nil {
			return 0, fmt.Errorf("missing frontmatter")
		}
		if markdown.FMString(ch.Frontmatter, "id") == "" {
			ch.Frontmatter["id"] = ch.ID
		}
		path := h.absItemPath(ch.Path)
		if path == "" {
			typ := markdown.FMString(ch.Frontmatter, "type")
			if typ == "" {
				typ = "note"
				ch.Frontmatter["type"] = typ
			}
			title := markdown.FMString(ch.Frontmatter, "title")
			path = filepath.Join(h.BaseDir, typ, time.Now().Format("2006"), time.Now().Format("01"), markdown.Slugify(title)+".md")
		}
		return h.appRevision(), h.Store.CreateItem(path, ch.Frontmatter, ch.Body)
	default:
		return 0, fmt.Errorf("unsupported operation %q", ch.Op)
	}
}

func (h *Handlers) relItemPath(path string) string {
	if path == "" {
		return ""
	}
	rel, err := filepath.Rel(h.BaseDir, path)
	if err != nil || strings.HasPrefix(rel, "..") {
		return filepath.ToSlash(path)
	}
	return filepath.ToSlash(rel)
}

func (h *Handlers) absItemPath(path string) string {
	if path == "" {
		return ""
	}
	clean := filepath.Clean(filepath.FromSlash(path))
	if filepath.IsAbs(clean) {
		return clean
	}
	if strings.HasPrefix(clean, "..") {
		return ""
	}
	return filepath.Join(h.BaseDir, clean)
}

func writeAppJSON(w http.ResponseWriter, v interface{}) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(http.StatusOK)
	_ = json.NewEncoder(w).Encode(v)
}

func renderAppRawMarkdown(fm map[string]interface{}, body string) string {
	var b strings.Builder
	b.WriteString("---\n")
	enc := yaml.NewEncoder(&b)
	enc.SetIndent(2)
	if err := enc.Encode(fm); err != nil {
		return body
	}
	b.WriteString("---\n")
	b.WriteString(body)
	return b.String()
}
