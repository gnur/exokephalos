package handlers

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	gotemplate "text/template"
	"time"

	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/id"
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
		svFilter := viewCfg.Subviews[subviewIdx].Filter
		if svFilter != "true" {
			svName := viewCfg.Subviews[subviewIdx].Name
			prog := h.subviewFilters[viewID+"\x00"+svName]
			if prog != nil {
				var filtered []scanner.Item
				for _, item := range items {
					ok, _ := prog.Eval(item.Frontmatter)
					if ok {
						filtered = append(filtered, item)
					}
				}
				items = filtered
			}
		}
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

	newFm, err := act.Mutate(item.Frontmatter)
	if err != nil {
		http.Error(w, fmt.Sprintf("action failed: %v", err), 500)
		return
	}
	if err := h.Store.UpdateItem(item.Path, newFm, item.Body); err != nil {
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

	// Parse template to extract variable names for the form
	vars := extractTemplateVars(viewCfg.Template)
	data["FormVars"] = vars

	h.render(w, r, "views/new.html", data)
}

// ViewNewPost handles the creation of a new item from form submission.
func (h *Handlers) ViewNewPost(w http.ResponseWriter, r *http.Request) {
	viewID := r.PathValue("viewId")
	viewCfg, ok := h.Cfg.Views[viewID]
	if !ok {
		http.NotFound(w, r)
		return
	}

	_ = r.ParseForm()

	// Build vars from form values
	vars := make(map[string]string)
	for key, values := range r.Form {
		if len(values) > 0 {
			vars[key] = values[0]
		}
	}

	// Import auto-fill vars from the TUI's create logic.
	// We duplicate the time-based fills here for the web.
	now := time.Now()
	if vars["Date"] == "" {
		vars["Date"] = now.Format("2006-01-02")
	}
	if vars["DateTime"] == "" {
		vars["DateTime"] = now.Format("2006-01-02T15:04:05")
	}
	if vars["Year"] == "" {
		vars["Year"] = now.Format("2006")
	}
	if vars["Month"] == "" {
		vars["Month"] = now.Format("01")
	}
	if vars["Day"] == "" {
		vars["Day"] = now.Format("02")
	}
	if vars["ID"] == "" {
		vars["ID"] = id.GenerateID()
	}

	// Render content and path from templates
	content, fullPath, err := renderWebCreateTemplate(viewCfg.Template, viewID, h.BaseDir, vars)
	if err != nil {
		http.Error(w, "Template error: "+err.Error(), 500)
		return
	}

	fm, body, err := markdown.ParseFrontmatterBytes([]byte(content))
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	if err := h.Store.CreateItem(fullPath, fm, body); err != nil {
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

// extractTemplateVars parses a Go template string and extracts {{.VarName}} references.
// It returns only the vars that are NOT auto-fill (Date, DateTime, ID, Year, Month, Day, Slug).
func extractTemplateVars(tmpl string) []string {
	autoFill := map[string]bool{
		"Date": true, "DateTime": true, "ID": true,
		"Year": true, "Month": true, "Day": true, "Slug": true,
	}

	seen := map[string]bool{}
	var vars []string

	// Simple regex-free extraction: find {{.Name}} patterns
	rest := tmpl
	for {
		idx := strings.Index(rest, "{{.")
		if idx == -1 {
			break
		}
		rest = rest[idx+3:]
		end := strings.Index(rest, "}}")
		if end == -1 {
			break
		}
		name := strings.TrimSpace(rest[:end])
		// Handle pipe expressions like {{.Name | func}}
		if pipeIdx := strings.Index(name, "|"); pipeIdx != -1 {
			name = strings.TrimSpace(name[:pipeIdx])
		}
		// Handle method calls
		if strings.Contains(name, "(") || strings.Contains(name, ".") {
			rest = rest[end+2:]
			continue
		}
		if name != "" && !autoFill[name] && !seen[name] {
			seen[name] = true
			vars = append(vars, name)
		}
		rest = rest[end+2:]
	}
	return vars
}

// renderWebCreateTemplate renders the content template and generates the file path.
// It ensures the resulting content always has 'id', 'type', 'tags', and 'created' fields in the frontmatter.
func renderWebCreateTemplate(contentTmpl, viewID, baseDir string, vars map[string]string) (string, string, error) {
	// Add Slug derived from Title
	title := vars["Title"]
	if title == "" {
		for k, v := range vars {
			if strings.ToLower(k) == "title" {
				title = v
				break
			}
		}
	}
	if title != "" {
		vars["Slug"] = markdown.Slugify(title)
	}

	// Ensure ID is available
	idVal, ok := vars["ID"]
	if !ok || idVal == "" {
		idVal = id.GenerateID()
		vars["ID"] = idVal
	}

	// Render content
	content, err := renderWebTemplate("content", contentTmpl, vars)
	if err != nil {
		return "", "", fmt.Errorf("rendering content template: %w", err)
	}

	defaultType := strings.TrimSuffix(viewID, "s")

	// Ensure id, type, tags, created are present in frontmatter
	content, err = markdown.EnsureRequiredFields(content, idVal, defaultType)
	if err != nil {
		return "", "", fmt.Errorf("ensuring required fields: %w", err)
	}

	// Generate destination path according to import logic
	absBase, err := filepath.Abs(baseDir)
	if err != nil {
		return "", "", fmt.Errorf("absolute base path: %w", err)
	}
	destDir := filepath.Join(absBase, idVal[:3])
	var fileName string
	if title != "" {
		slug := markdown.Slugify(title)
		if slug != "" {
			fileName = idVal + "-" + slug + ".md"
		} else {
			fileName = idVal + ".md"
		}
	} else {
		fileName = idVal + ".md"
	}
	fullPath := filepath.Clean(filepath.Join(destDir, fileName))

	if !strings.HasPrefix(fullPath, absBase+string(filepath.Separator)) && fullPath != absBase {
		return "", "", fmt.Errorf("path traversal detected: target path %s is outside base directory %s", fullPath, absBase)
	}

	return content, fullPath, nil
}

func renderWebTemplate(name, tmplStr string, vars map[string]string) (string, error) {
	t, err := gotemplate.New(name).Parse(tmplStr)
	if err != nil {
		return "", err
	}
	var buf strings.Builder
	if err := t.Execute(&buf, vars); err != nil {
		return "", err
	}
	return buf.String(), nil
}

// OrderedView re-export for templates.
type OrderedView = config.OrderedView
