package handlers

import (
	"fmt"
	"net/http"
	"os"
	"sort"
	"strings"
	"time"

	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/encryption"
	"github.com/gnur/exokephalos/internal/itemcreate"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/scanner"
	"gopkg.in/yaml.v3"
)

// ViewList handles GET /views/{viewId} — lists items matching the view filter.
func (h *Handlers) ViewList(w http.ResponseWriter, r *http.Request) {
	viewID := r.PathValue("viewId")
	viewCfg, ok := h.Cfg.Views[viewID]
	if !ok {
		http.NotFound(w, r)
		return
	}

	data := newData(r)
	parseStart := time.Now()

	items, err := h.scanAndFilter(viewID)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}

	// Apply subview filter if specified
	subviewIdx := 0
	if sv := r.URL.Query().Get("subview"); sv != "" {
		for i, s := range viewCfg.Subviews {
			if s.Name == sv {
				subviewIdx = i
				break
			}
		}
	}
	if subviewIdx > 0 || r.URL.Query().Get("subview") != "" {
		var filtered []scanner.Item
		for _, item := range items {
			ok, err := h.Cfg.MatchSubview(viewID, subviewIdx, config.Note{ID: item.ID, Path: item.Path, Type: item.Type, Tags: item.Tags, Frontmatter: item.Frontmatter, Body: item.Body})
			if err != nil {
				http.Error(w, err.Error(), 500)
				return
			}
			if ok {
				filtered = append(filtered, item)
			}
		}
		items = filtered
	}

	// Apply tag filtering if view has show_tags and tags param present
	activeTags := parseTags(r)
	if viewCfg.ShowTags && len(activeTags) > 0 {
		var filtered []scanner.Item
		for _, item := range items {
			if itemHasAllTags(item, activeTags) {
				filtered = append(filtered, item)
			}
		}
		items = filtered
	}

	// Sort items
	sortItems(items, viewCfg.SortField, viewCfg.SortOrder)

	// Group items by year for dividers
	yearGroups := groupByYear(items, viewCfg.SortField)

	data["_parseTime"] = time.Since(parseStart)
	data["Items"] = items
	data["YearGroups"] = yearGroups
	data["View"] = viewCfg
	data["ViewID"] = viewID
	data["Subviews"] = viewCfg.Subviews
	data["ActiveSubview"] = viewCfg.Subviews[subviewIdx].Name
	data["ActiveTags"] = activeTags

	// Compute tag counts if view shows tags
	if viewCfg.ShowTags {
		data["TagCounts"] = computeTagCounts(items)
	}

	h.render(w, r, "views/list.html", data)
}

// ViewDetail handles GET /views/{viewId}/{itemId} — shows a single item.
func (h *Handlers) ViewDetail(w http.ResponseWriter, r *http.Request) {
	viewID := r.PathValue("viewId")
	itemID := r.PathValue("itemId")

	viewCfg, ok := h.Cfg.Views[viewID]
	if !ok {
		http.NotFound(w, r)
		return
	}

	data := newData(r)
	parseStart := time.Now()

	items, err := h.scanAndFilter(viewID)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}

	item, found := h.findItem(items, itemID)
	data["_parseTime"] = time.Since(parseStart)
	if !found {
		http.NotFound(w, r)
		return
	}

	data["Item"] = item
	data["View"] = viewCfg
	data["ViewID"] = viewID
	data["ItemID"] = itemID
	data["Title"] = item.Title(viewCfg.TitleField)
	data["Body"] = item.Body

	var fmYAML string
	if item.Frontmatter != nil {
		if yb, err := yaml.Marshal(item.Frontmatter); err == nil {
			fmYAML = string(yb)
		}
	}
	data["FrontmatterYAML"] = fmYAML

	h.render(w, r, "views/detail.html", data)
}

// ViewEdit handles GET/POST /views/{viewId}/edit/{itemId} — raw editor.
func (h *Handlers) ViewEdit(w http.ResponseWriter, r *http.Request) {
	viewID := r.PathValue("viewId")
	itemID := r.PathValue("itemId")

	viewCfg, ok := h.Cfg.Views[viewID]
	if !ok {
		http.NotFound(w, r)
		return
	}

	items, err := h.scanAndFilter(viewID)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}

	item, found := h.findItem(items, itemID)
	if !found {
		http.NotFound(w, r)
		return
	}

	if r.Method == http.MethodPost {
		_ = r.ParseForm()
		content := r.FormValue("content")
		if err := h.Store.WriteRaw(item.Path, content); err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		http.Redirect(w, r, fmt.Sprintf("/views/%s/%s", viewID, itemID), http.StatusSeeOther)
		return
	}

	data := newData(r)
	parseStart := time.Now()
	raw, err := h.Store.ReadRaw(item.Path)
	data["_parseTime"] = time.Since(parseStart)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	data["Content"] = raw
	data["BackURL"] = fmt.Sprintf("/views/%s/%s", viewID, itemID)
	data["PostURL"] = fmt.Sprintf("/views/%s/edit/%s", viewID, itemID)
	data["DeleteURL"] = fmt.Sprintf("/views/%s/delete/%s", viewID, itemID)
	data["Title"] = "Edit: " + item.Title(viewCfg.TitleField)
	h.render(w, r, "edit.html", data)
}

// ViewDelete handles POST /views/{viewId}/delete/{itemId}.
func (h *Handlers) ViewDelete(w http.ResponseWriter, r *http.Request) {
	viewID := r.PathValue("viewId")
	itemID := r.PathValue("itemId")

	if _, ok := h.Cfg.Views[viewID]; !ok {
		http.NotFound(w, r)
		return
	}

	items, err := h.scanAndFilter(viewID)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}

	item, found := h.findItem(items, itemID)
	if !found {
		http.NotFound(w, r)
		return
	}

	if err := h.Store.DeleteItem(item.Path); err != nil && !os.IsNotExist(err) {
		http.Error(w, fmt.Sprintf("failed to delete item: %v", err), http.StatusInternalServerError)
		return
	}
	http.Redirect(w, r, fmt.Sprintf("/views/%s", viewID), http.StatusSeeOther)
}

// ViewAction handles POST /views/{viewId}/items/{itemId}/actions/{actionName}
func (h *Handlers) ViewAction(w http.ResponseWriter, r *http.Request) {
	viewID := r.PathValue("viewId")
	itemID := r.PathValue("itemId")
	actionName := r.PathValue("actionName")

	act, ok := h.actions[actionName]
	if !ok {
		http.Error(w, "action not found", 404)
		return
	}

	if _, ok := h.Cfg.Views[viewID]; !ok {
		http.NotFound(w, r)
		return
	}

	items, err := h.scanAndFilter(viewID)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}

	item, found := h.findItem(items, itemID)
	if !found {
		http.NotFound(w, r)
		return
	}

	replacement, err := act.Run(config.Note{ID: item.ID, Path: item.Path, Type: item.Type, Tags: item.Tags, Frontmatter: item.Frontmatter, Body: item.Body})
	if err != nil {
		http.Error(w, fmt.Sprintf("action failed: %v", err), 500)
		return
	}
	if err := h.Store.UpdateItem(item.Path, replacement.Frontmatter, replacement.Body); err != nil {
		http.Error(w, fmt.Sprintf("action failed: %v", err), 500)
		return
	}

	http.Redirect(w, r, fmt.Sprintf("/views/%s", viewID), http.StatusSeeOther)
}

// ViewNew handles GET/POST /views/{viewId}/new — create form.
func (h *Handlers) ViewNew(w http.ResponseWriter, r *http.Request) {
	viewID := r.PathValue("viewId")
	viewCfg, ok := h.Cfg.Views[viewID]
	if !ok {
		http.NotFound(w, r)
		return
	}

	data := newData(r)
	data["_parseTime"] = time.Duration(0)
	data["View"] = viewCfg
	data["ViewID"] = viewID

	if r.Method == http.MethodPost {
		// Handled by ViewNewPost
		h.ViewNewPost(w, r)
		return
	}

	h.render(w, r, "views/new.html", data)
}

// ViewNewPost handles the creation of a new item from form submission.
func (h *Handlers) ViewNewPost(w http.ResponseWriter, r *http.Request) {
	viewID := r.PathValue("viewId")
	_, ok := h.Cfg.Views[viewID]
	if !ok {
		http.NotFound(w, r)
		return
	}

	_ = r.ParseForm()

	item, err := itemcreate.New(h.BaseDir, r.FormValue("type"), r.FormValue("title"), r.FormValue("body"))
	if err != nil {
		http.Error(w, "Create error: "+err.Error(), http.StatusBadRequest)
		return
	}
	if err := itemcreate.Verify(item.Frontmatter, strings.TrimSpace(r.FormValue("type")), strings.TrimSpace(r.FormValue("title"))); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	fm, body := item.Frontmatter, item.Body
	if r.FormValue("encrypted") == "true" {
		passphrase := r.FormValue("passphrase")
		if passphrase == "" {
			http.Error(w, "an encryption passphrase is required", http.StatusBadRequest)
			return
		}
		noteID := markdown.FMString(fm, "id")
		if noteID == "" {
			http.Error(w, "encrypted notes require an id", http.StatusInternalServerError)
			return
		}
		body, err = encryption.Encrypt(noteID, passphrase, body)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		fm["encrypted"] = true
	}
	if err := h.Store.CreateItem(item.Path, fm, body); err != nil {
		http.Error(w, err.Error(), 500)
		return
	}

	http.Redirect(w, r, fmt.Sprintf("/views/%s", viewID), http.StatusSeeOther)
}

// --- Helper functions ---

func parseTags(r *http.Request) []string {
	tagsParam := r.URL.Query().Get("tags")
	if tagsParam == "" {
		tag := r.URL.Query().Get("tag")
		if tag != "" {
			return []string{tag}
		}
		return nil
	}
	var tags []string
	for _, t := range strings.Split(tagsParam, ",") {
		t = strings.TrimSpace(t)
		if t != "" {
			tags = append(tags, t)
		}
	}
	return tags
}

func itemHasAllTags(item scanner.Item, tags []string) bool {
	itemTags := item.GetTags()
	for _, needed := range tags {
		found := false
		for _, t := range itemTags {
			if t == needed {
				found = true
				break
			}
		}
		if !found {
			return false
		}
	}
	return true
}

func sortItems(items []scanner.Item, field, order string) {
	desc := order == "desc"
	sort.SliceStable(items, func(i, j int) bool {
		return itemLess(items[i], items[j], field, desc)
	})
}

func itemLess(a, b scanner.Item, field string, desc bool) bool {
	av := a.SortValue(field)
	bv := b.SortValue(field)
	if av != bv {
		if desc {
			return av > bv
		}
		return av < bv
	}

	aid := a.SortID()
	bid := b.SortID()
	if aid != bid {
		return aid < bid
	}
	return a.Path < b.Path
}

type TagCount struct {
	Tag   string
	Count int
}

// YearGroup groups items under a year label for display with dividers.
type YearGroup struct {
	Year  string
	Items []scanner.Item
}

func groupByYear(items []scanner.Item, sortField string) []YearGroup {
	var groups []YearGroup
	currentYear := ""
	for _, item := range items {
		year := item.Year(sortField)
		if year == "" {
			year = "Unknown"
		}
		if year != currentYear {
			groups = append(groups, YearGroup{Year: year})
			currentYear = year
		}
		groups[len(groups)-1].Items = append(groups[len(groups)-1].Items, item)
	}
	return groups
}

func computeTagCounts(items []scanner.Item) []TagCount {
	counts := map[string]int{}
	for _, item := range items {
		for _, t := range item.GetTags() {
			counts[t]++
		}
	}
	result := make([]TagCount, 0, len(counts))
	for t, c := range counts {
		result = append(result, TagCount{Tag: t, Count: c})
	}
	sort.Slice(result, func(i, j int) bool {
		if result[i].Count != result[j].Count {
			return result[i].Count > result[j].Count
		}
		return result[i].Tag < result[j].Tag
	})
	return result
}

// OrderedView re-export for templates.
type OrderedView = config.OrderedView
