package tui

import (
	"fmt"
	"log"
	"os"
	"os/exec"
	"sort"
	"strings"
	"text/template"

	"github.com/charmbracelet/bubbles/textinput"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/glamour"
	"github.com/gnur/exokephalos/internal/action"
	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/filter"
	"github.com/gnur/exokephalos/internal/goodreads"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/scanner"
	"gopkg.in/yaml.v3"
)

type mode int

const (
	modeNormal mode = iota
	modeViewMenu
	modeActionMenu
	modeSearchTags
	modeSearchItems
	modeConfirmDelete
	modeCreatePrompt
	modeImportURL
)

// Pane focus for views with tags enabled
type pane int

const (
	paneTags pane = iota
	paneList
	panePreview
)

type tagCountsCache struct {
	valid  bool
	counts []tagCountEntry
}

// viewState holds the runtime state for a single configured view.
type viewState struct {
	cfg            config.ViewConfig
	id             string
	filter         *filter.Program
	subFilters     []*filter.Program
	activeSubview  int
	items          []scanner.Item // all items matching parent filter
	filteredItems  []scanner.Item // items matching current subview
	cursor         int
	offset         int
	selectedTags   []string
	tagCursor      int
	tagOffset      int
	tagFilterValue string
	textFilter     string
	tagCountsCache *tagCountsCache
	previewTmpl    *template.Template
}

// Model is the Bubbletea model for the TUI.
type Model struct {
	cfg     *config.Config
	baseDir string
	cache   *cache.Cache
	width   int
	height  int
	ready   bool

	// View state
	views      []viewState
	activeView int

	// Pane focus (for views with tags)
	pane pane

	// Inputs
	tagFilterInput  textinput.Model
	textFilterInput textinput.Model
	promptInput     textinput.Model
	importInput     textinput.Model

	// Create flow
	createVars       map[string]string
	pendingPrompts   []string
	currentPromptIdx int

	// Preview
	preview viewport.Model

	// Mode
	mode mode

	// Action menu state
	actions     map[string]*action.Action
	actionItems []string // action names applicable to current item

	// Status message
	status string
}

type refreshMsg struct{}

type dataLoadedMsg struct {
	allItems []scanner.Item
}

func New(cfg *config.Config, baseDir string, c *cache.Cache) Model {
	tagFilter := textinput.New()
	tagFilter.Prompt = "filter tags: "
	tagFilter.CharLimit = 256

	textFilter := textinput.New()
	textFilter.Prompt = "filter: "
	textFilter.CharLimit = 256

	promptTi := textinput.New()
	promptTi.CharLimit = 512

	importTi := textinput.New()
	importTi.Prompt = "Goodreads URL: "
	importTi.CharLimit = 512

	// Initialize view states from config
	orderedViews := cfg.OrderedViews()
	views := make([]viewState, 0, len(orderedViews))

	for _, ov := range orderedViews {
		vs := viewState{
			cfg:            ov.Config,
			id:             ov.ID,
			tagCountsCache: &tagCountsCache{},
		}

		// Compile parent filter
		prog, err := filter.Compile(ov.Config.Filter)
		if err != nil {
			// If filter fails to compile, skip this view
			continue
		}
		vs.filter = prog

		// Compile subview filters
		for _, sv := range ov.Config.Subviews {
			subProg, err := filter.Compile(sv.Filter)
			if err != nil {
				// Use a permissive filter on error
				subProg, _ = filter.Compile("true")
			}
			vs.subFilters = append(vs.subFilters, subProg)
		}

		// Precompile preview template if configured
		if ov.Config.PreviewTemplate != "" {
			tmpl, err := template.New("preview-" + ov.ID).Parse(ov.Config.PreviewTemplate)
			if err != nil {
				log.Printf("tui: failed to compile preview template for view %s: %v", ov.ID, err)
			} else {
				vs.previewTmpl = tmpl
			}
		}

		views = append(views, vs)
	}

	// Compile actions from config
	actions := make(map[string]*action.Action)
	for name, ac := range cfg.Actions {
		act, err := action.Compile(name, ac)
		if err != nil {
			continue
		}
		actions[name] = act
	}

	return Model{
		cfg:             cfg,
		baseDir:         baseDir,
		cache:           c,
		views:           views,
		activeView:      cfg.DefaultViewIndex(),
		pane:            paneList,
		tagFilterInput:  tagFilter,
		textFilterInput: textFilter,
		promptInput:     promptTi,
		importInput:     importTi,
		actions:         actions,
		mode:            modeNormal,
	}
}

func Run(cfg *config.Config, baseDir string, c *cache.Cache) error {
	p := tea.NewProgram(New(cfg, baseDir, c), tea.WithAltScreen())
	_, err := p.Run()
	return err
}

func (m Model) Init() tea.Cmd {
	return m.loadData()
}

func (m Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height
		if !m.ready {
			m.preview = viewport.New(m.previewWidth(), m.contentHeight())
			m.ready = true
		} else {
			m.preview.Width = m.previewWidth()
			m.preview.Height = m.contentHeight()
		}
		m.updatePreview()
		return m, nil

	case refreshMsg:
		return m, m.loadData()

	case dataLoadedMsg:
		m.invalidateAllTagCounts()
		m.applyFilters(msg.allItems)
		m.clampCursor()
		m.updatePreview()
		return m, nil

	case tea.KeyMsg:
		return m.handleKey(msg)
	}

	return m, nil
}

func (m Model) handleKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	// Global quit
	if msg.String() == "ctrl+c" {
		return m, tea.Quit
	}

	switch m.mode {
	case modeViewMenu:
		return m.handleViewMenuKey(msg)
	case modeActionMenu:
		return m.handleActionMenuKey(msg)
	case modeConfirmDelete:
		return m.handleConfirmDeleteKey(msg)
	case modeCreatePrompt:
		return m.handleCreatePromptKey(msg)
	case modeImportURL:
		return m.handleImportKey(msg)
	case modeSearchTags:
		return m.handleSearchTagsKey(msg)
	case modeSearchItems:
		return m.handleSearchItemsKey(msg)
	case modeNormal:
		return m.handleNormalKey(msg)
	}

	return m, nil
}

func (m Model) handleViewMenuKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	if msg.String() == "esc" {
		m.mode = modeNormal
		return m, nil
	}

	// Match key to a view
	key := msg.String()
	for i, v := range m.views {
		if v.cfg.Key == key {
			m.switchView(i)
			return m, nil
		}
	}

	return m, nil
}

func (m Model) handleActionMenuKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "esc":
		m.mode = modeNormal
		return m, nil
	case "i":
		m.mode = modeImportURL
		m.importInput.SetValue("")
		m.importInput.Focus()
		return m, textinput.Blink
	default:
		key := msg.String()
		for _, name := range m.actionItems {
			if len(name) > 0 && string(name[0]) == key {
				return m.applyAction(name)
			}
		}
	}
	return m, nil
}

func (m *Model) populateActionItems() {
	m.actionItems = nil
	items := m.currentItems()
	vs := m.currentView()
	if vs == nil || len(items) == 0 || vs.cursor >= len(items) {
		return
	}
	item := items[vs.cursor]
	for name, act := range m.actions {
		if act.Match(item.Frontmatter) {
			m.actionItems = append(m.actionItems, name)
		}
	}
}

func (m Model) applyAction(actionName string) (tea.Model, tea.Cmd) {
	act, ok := m.actions[actionName]
	if !ok {
		m.status = "Action not found"
		m.mode = modeNormal
		return m, nil
	}

	items := m.currentItems()
	vs := m.currentView()
	if vs == nil || len(items) == 0 || vs.cursor >= len(items) {
		m.status = "No item selected"
		m.mode = modeNormal
		return m, nil
	}
	item := items[vs.cursor]

	if err := act.Apply(item.Path, item.Frontmatter, item.Body); err != nil {
		m.status = fmt.Sprintf("Action failed: %v", err)
	} else {
		_ = m.cache.NotifyWrite(item.Path)
		m.status = fmt.Sprintf("Applied: %s", act.Description)
	}

	m.mode = modeNormal
	return m, m.loadData()
}

func (m Model) handleConfirmDeleteKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "y", "Y":
		m.deleteSelected()
		m.mode = modeNormal
		return m, m.loadData()
	default:
		m.mode = modeNormal
		m.status = ""
		return m, nil
	}
}

func (m Model) handleCreatePromptKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "enter":
		// Save current prompt value
		val := m.promptInput.Value()
		if m.currentPromptIdx < len(m.pendingPrompts) {
			m.createVars[m.pendingPrompts[m.currentPromptIdx]] = val
		}
		m.currentPromptIdx++

		// Check if more prompts remain
		if m.currentPromptIdx < len(m.pendingPrompts) {
			nextVar := m.pendingPrompts[m.currentPromptIdx]
			m.promptInput.SetValue("")
			m.promptInput.Prompt = nextVar + ": "
			return m, nil
		}

		// All prompts done - create the file
		m.promptInput.Blur()
		m.mode = modeNormal
		return m.finishCreate()

	case "esc":
		m.mode = modeNormal
		m.promptInput.Blur()
		m.createVars = nil
		m.pendingPrompts = nil
		return m, nil

	default:
		var cmd tea.Cmd
		m.promptInput, cmd = m.promptInput.Update(msg)
		return m, cmd
	}
}

func (m Model) handleImportKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "enter":
		url := m.importInput.Value()
		if url == "" {
			m.mode = modeNormal
			return m, nil
		}
		m.importInput.Blur()
		m.mode = modeNormal
		return m.importBook(url)
	case "esc":
		m.mode = modeNormal
		m.importInput.Blur()
		m.importInput.SetValue("")
		return m, nil
	default:
		var cmd tea.Cmd
		m.importInput, cmd = m.importInput.Update(msg)
		return m, cmd
	}
}

func (m Model) handleSearchTagsKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	m.invalidateTagCounts()
	switch msg.String() {
	case "enter":
		m.mode = modeNormal
		m.tagFilterInput.Blur()
		if m.activeView < len(m.views) {
			m.views[m.activeView].tagFilterValue = m.tagFilterInput.Value()
			m.views[m.activeView].tagCursor = 0
			m.views[m.activeView].tagOffset = 0
		}
		return m, nil
	case "esc":
		m.mode = modeNormal
		m.tagFilterInput.Blur()
		m.tagFilterInput.SetValue("")
		if m.activeView < len(m.views) {
			m.views[m.activeView].tagFilterValue = ""
			m.views[m.activeView].tagCursor = 0
			m.views[m.activeView].tagOffset = 0
		}
		return m, nil
	default:
		var cmd tea.Cmd
		m.tagFilterInput, cmd = m.tagFilterInput.Update(msg)
		if m.activeView < len(m.views) {
			m.views[m.activeView].tagFilterValue = m.tagFilterInput.Value()
			m.views[m.activeView].tagCursor = 0
			m.views[m.activeView].tagOffset = 0
		}
		return m, cmd
	}
}

func (m Model) handleSearchItemsKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	m.invalidateTagCounts()
	switch msg.String() {
	case "enter":
		m.mode = modeNormal
		m.textFilterInput.Blur()
		if m.activeView < len(m.views) {
			m.views[m.activeView].textFilter = m.textFilterInput.Value()
			m.views[m.activeView].cursor = 0
			m.views[m.activeView].offset = 0
		}
		m.clampCursor()
		m.updatePreview()
		return m, nil
	case "esc":
		m.mode = modeNormal
		m.textFilterInput.Blur()
		m.textFilterInput.SetValue("")
		if m.activeView < len(m.views) {
			m.views[m.activeView].textFilter = ""
			m.views[m.activeView].cursor = 0
			m.views[m.activeView].offset = 0
		}
		m.clampCursor()
		m.updatePreview()
		return m, nil
	default:
		var cmd tea.Cmd
		m.textFilterInput, cmd = m.textFilterInput.Update(msg)
		if m.activeView < len(m.views) {
			m.views[m.activeView].textFilter = m.textFilterInput.Value()
			m.views[m.activeView].cursor = 0
			m.views[m.activeView].offset = 0
		}
		m.clampCursor()
		m.updatePreview()
		return m, cmd
	}
}

func (m Model) handleNormalKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "q":
		return m, tea.Quit

	case "g":
		m.mode = modeViewMenu
		return m, nil

	case "a":
		m.populateActionItems()
		m.mode = modeActionMenu
		return m, nil

	// Tab/shift-tab cycles subviews
	case "tab":
		m.cycleSubview(1)
		return m, nil
	case "shift+tab":
		m.cycleSubview(-1)
		return m, nil

	// h/l - pane navigation or no-op
	case "h", "left":
		return m.handleHL(-1)
	case "l", "right":
		return m.handleHL(1)

	// j/k navigation
	case "j", "down":
		m.navigateDown()
		return m, nil
	case "k", "up":
		m.navigateUp()
		return m, nil

	case "G":
		m.navigateBottom()
		return m, nil

	// Space: toggle tag
	case " ":
		if m.currentViewShowsTags() && m.pane == paneTags {
			m.toggleTag()
		}
		return m, nil

	case "/":
		if m.currentViewShowsTags() && m.pane == paneTags {
			m.mode = modeSearchTags
			vs := m.currentView()
			if vs != nil {
				m.tagFilterInput.SetValue(vs.tagFilterValue)
			}
			m.tagFilterInput.Focus()
			return m, textinput.Blink
		}
		m.mode = modeSearchItems
		vs := m.currentView()
		if vs != nil {
			m.textFilterInput.SetValue(vs.textFilter)
		}
		m.textFilterInput.Focus()
		return m, textinput.Blink

	case "esc":
		vs := m.currentView()
		if vs == nil {
			return m, nil
		}
		m.invalidateTagCounts()
		if m.currentViewShowsTags() && m.pane == paneTags && vs.tagFilterValue != "" {
			vs.tagFilterValue = ""
			m.tagFilterInput.SetValue("")
			vs.tagCursor = 0
			vs.tagOffset = 0
		} else if vs.textFilter != "" {
			vs.textFilter = ""
			m.textFilterInput.SetValue("")
			vs.cursor = 0
			vs.offset = 0
			m.clampCursor()
			m.updatePreview()
		}
		return m, nil

	case "enter", "e":
		return m.editSelected()

	case "n":
		return m.createNew()

	case "d":
		items := m.currentItems()
		vs := m.currentView()
		if vs != nil && len(items) > 0 && vs.cursor < len(items) {
			title := items[vs.cursor].Title(vs.cfg.TitleField)
			m.mode = modeConfirmDelete
			m.status = fmt.Sprintf("Delete '%s'? (y/n)", title)
		}
		return m, nil
	}

	return m, nil
}

// --- View switching ---

func (m *Model) switchView(idx int) {
	if idx < 0 || idx >= len(m.views) {
		return
	}
	m.activeView = idx
	m.invalidateTagCounts()
	m.pane = paneList
	m.mode = modeNormal
	m.status = ""
	m.textFilterInput.SetValue("")
	m.tagFilterInput.SetValue("")
	m.clampCursor()
	m.updatePreview()
}

func (m *Model) cycleSubview(dir int) {
	vs := m.currentView()
	if vs == nil {
		return
	}
	m.invalidateTagCounts()
	numSubviews := len(vs.cfg.Subviews)
	if numSubviews <= 1 {
		return
	}
	vs.activeSubview = (vs.activeSubview + dir + numSubviews) % numSubviews
	m.refilterCurrentView()
	vs.cursor = 0
	vs.offset = 0
	m.clampCursor()
	m.updatePreview()
}

// --- Pane navigation ---

func (m Model) handleHL(dir int) (tea.Model, tea.Cmd) {
	if !m.currentViewShowsTags() {
		return m, nil
	}
	newPane := int(m.pane) + dir
	if newPane < int(paneTags) {
		newPane = int(paneTags)
	}
	if newPane > int(panePreview) {
		newPane = int(panePreview)
	}
	m.pane = pane(newPane)
	return m, nil
}

// --- Navigation ---

func (m *Model) navigateDown() {
	vs := m.currentView()
	if vs == nil {
		return
	}

	if m.currentViewShowsTags() && m.pane == paneTags {
		tagCounts := m.computeTagCounts()
		vs.tagCursor++
		if vs.tagCursor >= len(tagCounts) {
			vs.tagCursor = len(tagCounts) - 1
		}
		if vs.tagCursor < 0 {
			vs.tagCursor = 0
		}
		visible := m.contentHeight() - 1
		if vs.tagCursor >= vs.tagOffset+visible {
			vs.tagOffset = vs.tagCursor - visible + 1
		}
		return
	}

	vs.cursor++
	m.clampCursor()
	m.updatePreview()
}

func (m *Model) navigateUp() {
	vs := m.currentView()
	if vs == nil {
		return
	}

	if m.currentViewShowsTags() && m.pane == paneTags {
		vs.tagCursor--
		if vs.tagCursor < 0 {
			vs.tagCursor = 0
		}
		if vs.tagCursor < vs.tagOffset {
			vs.tagOffset = vs.tagCursor
		}
		return
	}

	vs.cursor--
	m.clampCursor()
	m.updatePreview()
}

func (m *Model) navigateBottom() {
	vs := m.currentView()
	if vs == nil {
		return
	}

	if m.currentViewShowsTags() && m.pane == paneTags {
		tagCounts := m.computeTagCounts()
		vs.tagCursor = len(tagCounts) - 1
		if vs.tagCursor < 0 {
			vs.tagCursor = 0
		}
		visible := m.contentHeight() - 1
		if vs.tagCursor >= visible {
			vs.tagOffset = vs.tagCursor - visible + 1
		}
		return
	}

	items := m.currentItems()
	vs.cursor = len(items) - 1
	m.clampCursor()
	m.updatePreview()
}

// --- Tag interaction ---

func (m *Model) toggleTag() {
	vs := m.currentView()
	if vs == nil {
		return
	}

	m.invalidateTagCounts()
	tagCounts := m.computeTagCounts()
	if vs.tagCursor >= len(tagCounts) {
		return
	}
	tag := tagCounts[vs.tagCursor].tag

	// Toggle selection
	var newTags []string
	found := false
	for _, t := range vs.selectedTags {
		if t == tag {
			found = true
		} else {
			newTags = append(newTags, t)
		}
	}
	if !found {
		newTags = append(newTags, tag)
	}
	vs.selectedTags = newTags
	m.invalidateTagCounts()

	// Recompute and keep cursor on same tag
	newTagCounts := m.computeTagCounts()
	for i, tc := range newTagCounts {
		if tc.tag == tag {
			vs.tagCursor = i
			break
		}
	}

	vs.cursor = 0
	vs.offset = 0
	m.clampCursor()
	m.updatePreview()
}

func (m Model) invalidateTagCounts() {
	vs := m.currentView()
	if vs != nil && vs.tagCountsCache != nil {
		vs.tagCountsCache.valid = false
	}
}

func (m Model) invalidateAllTagCounts() {
	for i := range m.views {
		if m.views[i].tagCountsCache != nil {
			m.views[i].tagCountsCache.valid = false
		}
	}
}

func (m Model) computeTagCounts() []tagCountEntry {
	vs := m.currentView()
	if vs == nil {
		return nil
	}

	if vs.tagCountsCache != nil && vs.tagCountsCache.valid {
		return vs.tagCountsCache.counts
	}

	// Get items filtered by tags (for computing counts on visible items)
	items := m.tagFilteredItems()

	// Also apply text filter if active
	if query := strings.ToLower(vs.textFilter); query != "" {
		items = m.textFilterItems(items, vs)
	}

	counts := make(map[string]int)
	for _, item := range items {
		for _, t := range item.GetTags() {
			counts[t]++
		}
	}

	// Ensure selected tags are always present
	for _, st := range vs.selectedTags {
		if _, ok := counts[st]; !ok {
			counts[st] = 0
		}
	}

	var result []tagCountEntry
	filterLower := strings.ToLower(vs.tagFilterValue)
	for t, c := range counts {
		if filterLower != "" && !strings.Contains(strings.ToLower(t), filterLower) {
			// Always keep selected tags visible
			isSelected := false
			for _, st := range vs.selectedTags {
				if st == t {
					isSelected = true
					break
				}
			}
			if !isSelected {
				continue
			}
		}
		result = append(result, tagCountEntry{tag: t, count: c})
	}
	sort.Slice(result, func(i, j int) bool {
		if result[i].count != result[j].count {
			return result[i].count > result[j].count
		}
		return result[i].tag < result[j].tag
	})

	if vs.tagCountsCache != nil {
		vs.tagCountsCache.counts = result
		vs.tagCountsCache.valid = true
	}
	return result
}

// --- Filtering ---

func (m *Model) applyFilters(allItems []scanner.Item) {
	for i := range m.views {
		vs := &m.views[i]
		vs.items = nil
		for _, item := range allItems {
			matched, err := vs.filter.Eval(item.Frontmatter)
			if err == nil && matched {
				vs.items = append(vs.items, item)
			}
		}
		// Sort items
		m.sortViewItems(vs)
		// Apply subview filter
		m.applySubviewFilter(vs)
	}
}

func (m *Model) sortViewItems(vs *viewState) {
	sortField := vs.cfg.SortField
	desc := vs.cfg.SortOrder == "desc"

	sort.SliceStable(vs.items, func(i, j int) bool {
		vi := markdown.FMString(vs.items[i].Frontmatter, sortField)
		vj := markdown.FMString(vs.items[j].Frontmatter, sortField)
		if desc {
			return vi > vj
		}
		return vi < vj
	})
}

func (m *Model) applySubviewFilter(vs *viewState) {
	if vs.activeSubview >= len(vs.subFilters) {
		vs.filteredItems = vs.items
		return
	}
	subFilter := vs.subFilters[vs.activeSubview]
	vs.filteredItems = nil
	for _, item := range vs.items {
		matched, err := subFilter.Eval(item.Frontmatter)
		if err == nil && matched {
			vs.filteredItems = append(vs.filteredItems, item)
		}
	}
}

func (m *Model) refilterCurrentView() {
	vs := m.currentView()
	if vs == nil {
		return
	}
	m.applySubviewFilter(vs)
}

// tagFilteredItems returns items from the current view that match selected tags.
func (m Model) tagFilteredItems() []scanner.Item {
	vs := m.currentView()
	if vs == nil {
		return nil
	}
	if len(vs.selectedTags) == 0 {
		return vs.filteredItems
	}
	var result []scanner.Item
	for _, item := range vs.filteredItems {
		if itemMatchesTags(item, vs.selectedTags) {
			result = append(result, item)
		}
	}
	return result
}

func (m Model) textFilterItems(items []scanner.Item, vs *viewState) []scanner.Item {
	query := strings.ToLower(vs.textFilter)
	if query == "" {
		return items
	}
	var result []scanner.Item
	for _, item := range items {
		title := strings.ToLower(item.Title(vs.cfg.TitleField))
		if strings.Contains(title, query) {
			result = append(result, item)
			continue
		}

		subtitle := strings.ToLower(item.Subtitle(vs.cfg.SubtitleField))
		if strings.Contains(subtitle, query) {
			result = append(result, item)
			continue
		}

		body := strings.ToLower(item.Body)
		if strings.Contains(body, query) {
			result = append(result, item)
			continue
		}
	}
	return result
}

// currentItems returns the visible items for the active view with all filters applied.
func (m Model) currentItems() []scanner.Item {
	vs := m.currentView()
	if vs == nil {
		return nil
	}

	items := m.tagFilteredItems()
	return m.textFilterItems(items, vs)
}

func itemMatchesTags(item scanner.Item, selectedTags []string) bool {
	tags := item.GetTags()
	for _, st := range selectedTags {
		found := false
		for _, t := range tags {
			if t == st {
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

// --- Actions ---

func (m Model) editSelected() (tea.Model, tea.Cmd) {
	items := m.currentItems()
	vs := m.currentView()
	if vs == nil || len(items) == 0 || vs.cursor >= len(items) {
		return m, nil
	}
	item := items[vs.cursor]
	if item.Path == "" {
		return m, nil
	}
	editor := os.Getenv("EDITOR")
	if editor == "" {
		editor = "vim"
	}
	c := exec.Command(editor, item.Path)
	return m, tea.ExecProcess(c, func(err error) tea.Msg {
		return refreshMsg{}
	})
}

func (m Model) importBook(url string) (tea.Model, tea.Cmd) {
	meta, err := goodreads.FetchBook(url)
	if err != nil {
		m.status = fmt.Sprintf("Import error: %v", err)
		return m, nil
	}

	// Start with auto-fill vars (Date, DateTime, ID, etc.)
	vars := newAutoFillVars()

	// Add book-specific fields
	vars["Title"] = meta.Title
	vars["Author"] = formatYAMLStringList(meta.Author)
	vars["URL"] = meta.URL
	vars["Pages"] = fmt.Sprintf("%d", meta.Pages)
	vars["Cover"] = meta.Cover

	// Find the books view to get its template and path
	for _, vs := range m.views {
		if strings.Contains(strings.ToLower(vs.cfg.Name), "book") {
			content, path, err := renderCreateTemplate(vs.cfg.Template, vs.id, m.baseDir, vars)
			if err != nil {
				m.status = fmt.Sprintf("Template error: %v", err)
				return m, nil
			}
			if err := writeNewFile(path, content); err != nil {
				m.status = fmt.Sprintf("Write error: %v", err)
				return m, nil
			}
			m.status = fmt.Sprintf("Imported: %s", meta.Title)
			return m, m.loadData()
		}
	}

	m.status = "No books view configured"
	return m, nil
}

func (m *Model) deleteSelected() {
	items := m.currentItems()
	vs := m.currentView()
	if vs == nil || len(items) == 0 || vs.cursor >= len(items) {
		return
	}
	item := items[vs.cursor]
	if item.Path != "" {
		if err := os.Remove(item.Path); err != nil && !os.IsNotExist(err) {
			m.status = fmt.Sprintf("Error deleting: %v", err)
			return
		}
	}
	m.status = ""
}

// --- Helpers ---

func (m *Model) currentView() *viewState {
	if m.activeView < 0 || m.activeView >= len(m.views) {
		return nil
	}
	return &m.views[m.activeView]
}

func (m Model) currentViewShowsTags() bool {
	vs := m.currentView()
	if vs == nil {
		return false
	}
	return vs.cfg.ShowTags
}

func (m *Model) clampCursor() {
	vs := m.currentView()
	if vs == nil {
		return
	}
	items := m.currentItems()
	if vs.cursor < 0 {
		vs.cursor = 0
	}
	if vs.cursor >= len(items) {
		vs.cursor = len(items) - 1
	}
	if vs.cursor < 0 {
		vs.cursor = 0
	}
	listH := m.contentHeight() - 1
	if vs.cursor < vs.offset {
		vs.offset = vs.cursor
	} else {
		for vs.offset < vs.cursor && linesNeeded(items, vs.offset, vs.cursor, vs) > listH {
			vs.offset++
		}
	}
}

// linesNeeded computes the number of terminal lines required to render
// items from index `start` up to (and including) index `end`.
func linesNeeded(items []scanner.Item, start, end int, vs *viewState) int {
	if start > end || start < 0 || end >= len(items) {
		return 0
	}
	linesCount := 0
	prevYear := ""
	if start > 0 && start < len(items) {
		prevYear = items[start].Year(vs.cfg.SortField)
	}

	for i := start; i <= end; i++ {
		item := items[i]
		year := item.Year(vs.cfg.SortField)
		if year == "" {
			year = "Unknown"
		}
		if year != prevYear {
			linesCount++ // for divider
			prevYear = year
		}
		linesCount++ // for title
		if vs.cfg.SubtitleField != "" {
			sub := item.Subtitle(vs.cfg.SubtitleField)
			if sub != "" {
				linesCount++ // for subtitle
			}
		}
	}
	return linesCount
}

func (m *Model) updatePreview() {
	items := m.currentItems()
	vs := m.currentView()
	if vs == nil || len(items) == 0 || vs.cursor >= len(items) {
		m.preview.SetContent("(no items)")
		return
	}
	item := items[vs.cursor]

	var content string

	if vs.previewTmpl != nil {
		content = m.renderPreviewTemplate(vs, &item)
	} else {
		yamlBytes, err := yaml.Marshal(item.Frontmatter)
		if err != nil {
			content = item.Body
		} else {
			content = "```yaml\n" + string(yamlBytes) + "```\n\n" + item.Body
		}
		if content == "" {
			content = item.Title(vs.cfg.TitleField)
		}
	}

	rendered, err := glamour.Render(content, "dark")
	if err == nil {
		content = rendered
	}
	m.preview.SetContent(content)
}

// renderPreviewTemplate executes a Go text/template against an item's data.
// Available fields: all frontmatter keys as .FieldName, plus .Body for the body content.
func (m *Model) renderPreviewTemplate(vs *viewState, item *scanner.Item) string {
	if vs.previewTmpl == nil {
		return ""
	}

	// Build template data: frontmatter fields + Body
	data := make(map[string]interface{}, len(item.Frontmatter)+1)
	for k, v := range item.Frontmatter {
		data[k] = v
	}
	data["Body"] = item.Body

	var buf strings.Builder
	if err := vs.previewTmpl.Execute(&buf, data); err != nil {
		return fmt.Sprintf("(render error: %v)", err)
	}
	return buf.String()
}

func (m Model) previewWidth() int {
	if m.currentViewShowsTags() {
		return m.width * 40 / 100
	}
	return m.width/2 - 2
}

func (m Model) listWidth() int {
	if m.currentViewShowsTags() {
		return m.width * 38 / 100
	}
	return m.width - m.previewWidth() - 3
}

func (m Model) tagPaneWidth() int {
	return m.width - m.listWidth() - m.previewWidth() - 4
}

func (m Model) contentHeight() int {
	return m.height - 4
}

// --- Data loading ---

func (m Model) loadData() tea.Cmd {
	c := m.cache
	return func() tea.Msg {
		items, err := c.All()
		if err != nil {
			return dataLoadedMsg{allItems: nil}
		}
		return dataLoadedMsg{allItems: items}
	}
}

// --- Utility ---


type tagCountEntry struct {
	tag   string
	count int
}
