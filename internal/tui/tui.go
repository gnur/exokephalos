package tui

import (
	"context"
	"fmt"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"text/template"
	"time"
	"unicode"

	"github.com/charmbracelet/bubbles/textinput"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/glamour"
	"github.com/gnur/exokephalos/internal/action"
	"github.com/gnur/exokephalos/internal/assets"
	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/encryption"
	"github.com/gnur/exokephalos/internal/filter"
	"github.com/gnur/exokephalos/internal/goodreads"
	"github.com/gnur/exokephalos/internal/hardcover"
	"github.com/gnur/exokephalos/internal/itemcreate"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/repo"
	"github.com/gnur/exokephalos/internal/scanner"
	"github.com/gnur/exokephalos/internal/urlimport"
	"gopkg.in/yaml.v3"
)

type mode int

const (
	modeNormal mode = iota
	modeViewMenu
	modeActionPicker
	modeSearchTags
	modeSearchItems
	modeConfirmDelete
	modeCreatePrompt
	modeImportURL
	modeHardcoverQuery
	modeHardcoverResults
	modeURLImport
	modeSyncOutbox
	modeAttachImage
	modeEncryptedEdit
	modeCreateEncrypted
	modeActionError
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
	cfg config.ViewConfig
	id  string
	// Deprecated test-only fields; runtime filtering uses config predicates.
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
	appCfg  *config.AppConfig
	width   int
	height  int
	ready   bool

	// View state
	views      []viewState
	activeView int

	// Pane focus (for views with tags)
	pane pane

	// Inputs
	tagFilterInput          textinput.Model
	textFilterInput         textinput.Model
	promptInput             textinput.Model
	importInput             textinput.Model
	hardcoverInput          textinput.Model
	urlInput                textinput.Model
	attachInput             textinput.Model
	encryptedEdit           *scanner.Item
	encryptedTemp           string
	encryptedPass           string
	pendingEncryptedContent string
	pendingEncryptedPath    string
	actionInput             textinput.Model
	viewMenuInput           string

	// Create flow
	createVars       map[string]string
	pendingPrompts   []string
	currentPromptIdx int
	createEncrypted  bool

	// Preview
	preview viewport.Model

	// Mode
	mode mode

	// Action picker state
	actions      map[string]*action.Action
	actionCursor int

	// Hardcover search flow
	hardcoverResults []hardcover.Book

	// Status message
	status      string
	actionError string

	syncStatus         string
	syncTickScheduled  bool
	syncStartScheduled bool
	syncOutboxFilter   string
	syncOutboxCursor   int
	syncOutboxOffset   int
	syncOutboxDetail   bool
}

type refreshMsg struct{}
type encryptedEditMsg struct{ err error }

type dataLoadedMsg struct {
	allItems []scanner.Item
}

type hardcoverSearchMsg struct {
	results []hardcover.Book
	err     error
}

type actionEntryKind int

const (
	actionEntryConfigured actionEntryKind = iota
	actionEntryGoodreads
	actionEntryHardcover
	actionEntryURL
	actionEntryStartSync
	actionEntrySyncOutbox
	actionEntryAttachImage
)

type actionEntry struct {
	Kind        actionEntryKind
	Name        string
	Description string
	Filter      string
	Enabled     bool
}

type viewShortcut struct {
	Key   string
	Label string
	Index int
}

type urlImportMsg struct {
	result urlimport.Result
	err    error
}

type syncMsg struct {
	status        string
	err           error
	startListen   bool
	retryListen   bool
	retrySync     bool
	retryStart    bool
	configChanged bool
}

type syncTickMsg struct{}
type syncStartTickMsg struct{}

func New(cfg *config.Config, baseDir string, c *cache.Cache, appCfg ...*config.AppConfig) Model {
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

	hardcoverTi := textinput.New()
	hardcoverTi.Prompt = "Hardcover query: "
	hardcoverTi.CharLimit = 512

	urlTi := textinput.New()
	urlTi.Prompt = "URL: "
	urlTi.CharLimit = 2048

	actionTi := textinput.New()
	actionTi.Prompt = ":"
	actionTi.CharLimit = 256

	attachTi := textinput.New()
	attachTi.Prompt = "Image path: "
	attachTi.CharLimit = 2048

	// Initialize view states from config
	orderedViews := cfg.OrderedViews()
	views := make([]viewState, 0, len(orderedViews))

	for _, ov := range orderedViews {
		vs := viewState{
			cfg:            ov.Config,
			id:             ov.ID,
			tagCountsCache: &tagCountsCache{},
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

	var ac *config.AppConfig
	if len(appCfg) > 0 {
		ac = appCfg[0]
	}
	syncStatus := ""
	if ac != nil && ac.Sync.ServerURL != "" {
		syncStatus = "not started"
		if c != nil && c.IsSyncStarted() {
			syncStatus = "offline"
		}
	}
	return Model{
		cfg:             cfg,
		baseDir:         baseDir,
		cache:           c,
		appCfg:          ac,
		views:           views,
		activeView:      cfg.DefaultViewIndex(),
		pane:            paneList,
		tagFilterInput:  tagFilter,
		textFilterInput: textFilter,
		promptInput:     promptTi,
		importInput:     importTi,
		hardcoverInput:  hardcoverTi,
		urlInput:        urlTi,
		actionInput:     actionTi,
		attachInput:     attachTi,
		actions:         actions,
		mode:            modeNormal,
		syncStatus:      syncStatus,
	}
}

func Run(cfg *config.Config, baseDir string, c *cache.Cache, appCfg *config.AppConfig) error {
	p := tea.NewProgram(New(cfg, baseDir, c, appCfg), tea.WithAltScreen())
	_, err := p.Run()
	return err
}

func (m Model) Init() tea.Cmd {
	if m.appCfg != nil && m.appCfg.Sync.ServerURL != "" && m.cache != nil && m.cache.IsSyncStarted() {
		return tea.Batch(m.loadData(), syncStartupCmd(m.baseDir, m.cache, m.appCfg))
	}
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
		return m, tea.Batch(m.loadData(), m.reconcileIfStartedCmd())
	case encryptedEditMsg:
		defer func() {
			if m.encryptedTemp != "" {
				_ = os.Remove(m.encryptedTemp)
			}
			m.encryptedTemp = ""
			m.encryptedPass = ""
			m.encryptedEdit = nil
		}()
		if msg.err != nil {
			m.status = fmt.Sprintf("Editor error: %v", msg.err)
			return m, nil
		}
		if m.encryptedEdit == nil {
			return m, nil
		}
		plain, err := os.ReadFile(m.encryptedTemp)
		if err != nil {
			m.status = fmt.Sprintf("Read encrypted edit: %v", err)
			return m, nil
		}
		fm, body, err := markdown.ParseFrontmatterBytes(plain)
		if err != nil {
			m.status = fmt.Sprintf("Parse encrypted edit: %v", err)
			return m, nil
		}
		noteID := markdown.FMString(fm, "id")
		if noteID == "" {
			m.status = "Encrypted notes require an id"
			return m, nil
		}
		fm["encrypted"] = true
		body, err = encryption.Encrypt(noteID, m.encryptedPass, body)
		if err != nil {
			m.status = fmt.Sprintf("Encrypt edit: %v", err)
			return m, nil
		}
		if err := markdown.WriteFrontmatter(m.encryptedEdit.Path, fm, body); err != nil {
			m.status = fmt.Sprintf("Save encrypted edit: %v", err)
			return m, nil
		}
		m.status = "Encrypted note saved"
		return m, tea.Batch(m.loadData(), m.reconcileIfStartedCmd())

	case dataLoadedMsg:
		m.invalidateAllTagCounts()
		m.applyFilters(msg.allItems)
		m.clampCursor()
		m.updatePreview()
		return m, nil

	case hardcoverSearchMsg:
		if msg.err != nil {
			m.status = fmt.Sprintf("Hardcover error: %v", msg.err)
			m.mode = modeNormal
			return m, nil
		}
		m.hardcoverResults = msg.results
		m.mode = modeHardcoverResults
		m.status = ""
		return m, nil

	case urlImportMsg:
		m.mode = modeNormal
		m.urlInput.Blur()
		if msg.err != nil {
			m.status = fmt.Sprintf("URL import error: %v", msg.err)
			return m, nil
		}
		m.status = fmt.Sprintf("Imported URL: %s", msg.result.Frontmatter["title"])
		return m, tea.Batch(m.loadData(), m.reconcileIfStartedCmd())

	case syncMsg:
		oldSyncStatus := m.syncStatus
		if msg.status != "" {
			m.syncStatus = msg.status
		}
		if msg.err != nil {
			m.status = fmt.Sprintf("Sync error: %v", msg.err)
			if m.syncStatus == "" {
				m.syncStatus = "error"
			}
		} else if msg.status != "" && msg.status != oldSyncStatus {
			m.status = "Sync: " + msg.status
		}
		if msg.configChanged {
			if err := m.reloadConfig(); err != nil {
				m.status = fmt.Sprintf("Config reload error: %v", err)
			}
		}
		var cmds []tea.Cmd
		cmds = append(cmds, m.loadData())
		if msg.startListen {
			cmds = append(cmds, syncListenCmd(m.baseDir, m.cache, m.appCfg))
		}
		if msg.retryListen {
			cmds = append(cmds, tea.Tick(5*time.Second, func(time.Time) tea.Msg {
				return syncMsg{startListen: true}
			}))
		}
		if msg.retrySync && m.cache != nil && m.cache.IsSyncStarted() && !m.syncTickScheduled {
			m.syncTickScheduled = true
			cmds = append(cmds, syncTickCmd(5*time.Second))
		}
		if msg.retryStart && !m.syncStartScheduled {
			m.syncStartScheduled = true
			cmds = append(cmds, syncStartTickCmd(5*time.Second))
		}
		return m, tea.Batch(cmds...)

	case syncTickMsg:
		m.syncTickScheduled = false
		if m.appCfg != nil && m.appCfg.Sync.ServerURL != "" && m.cache != nil && m.cache.IsSyncStarted() {
			return m, reconcileSyncCmd(m.baseDir, m.cache, m.appCfg)
		}
		return m, nil

	case syncStartTickMsg:
		m.syncStartScheduled = false
		if m.appCfg != nil && m.appCfg.Sync.ServerURL != "" && m.cache != nil && !m.cache.IsSyncStarted() {
			return m, startSyncCmd(m.baseDir, m.cache, m.appCfg)
		}
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
	case modeActionPicker:
		return m.handleActionPickerKey(msg)
	case modeActionError:
		return m.handleActionErrorKey(msg)
	case modeConfirmDelete:
		return m.handleConfirmDeleteKey(msg)
	case modeCreatePrompt:
		return m.handleCreatePromptKey(msg)
	case modeImportURL:
		return m.handleImportKey(msg)
	case modeHardcoverQuery:
		return m.handleHardcoverQueryKey(msg)
	case modeHardcoverResults:
		return m.handleHardcoverResultsKey(msg)
	case modeURLImport:
		return m.handleURLImportKey(msg)
	case modeSyncOutbox:
		return m.handleSyncOutboxKey(msg)
	case modeAttachImage:
		return m.handleAttachImageKey(msg)
	case modeEncryptedEdit:
		return m.handleEncryptedEditKey(msg)
	case modeCreateEncrypted:
		return m.handleCreateEncryptedKey(msg)
	case modeSearchTags:
		return m.handleSearchTagsKey(msg)
	case modeSearchItems:
		return m.handleSearchItemsKey(msg)
	case modeNormal:
		return m.handleNormalKey(msg)
	}

	return m, nil
}

func (m Model) reconcileIfStartedCmd() tea.Cmd {
	if m.appCfg == nil || m.cache == nil || m.appCfg.Sync.ServerURL == "" || !m.cache.IsSyncStarted() {
		return nil
	}
	return reconcileSyncCmd(m.baseDir, m.cache, m.appCfg)
}

func (m Model) handleViewMenuKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "esc":
		m.mode = modeNormal
		m.viewMenuInput = ""
		return m, nil
	case "backspace", "ctrl+h":
		if m.viewMenuInput != "" {
			runes := []rune(m.viewMenuInput)
			m.viewMenuInput = string(runes[:len(runes)-1])
		}
		return m, nil
	}

	key := msg.String()
	if len([]rune(key)) != 1 {
		return m, nil
	}
	r := []rune(key)[0]
	if !unicode.IsLetter(r) && !unicode.IsDigit(r) {
		return m, nil
	}
	m.viewMenuInput += strings.ToLower(string(r))

	shortcuts := m.viewShortcuts()
	hasPrefix := false
	for _, shortcut := range shortcuts {
		if shortcut.Key == m.viewMenuInput {
			m.switchView(shortcut.Index)
			return m, nil
		}
		if strings.HasPrefix(shortcut.Key, m.viewMenuInput) {
			hasPrefix = true
		}
	}
	if !hasPrefix {
		m.viewMenuInput = ""
	}

	return m, nil
}

func (m Model) viewShortcuts() []viewShortcut {
	sources := make([]string, len(m.views))
	for i, v := range m.views {
		source := normalizeViewShortcutSource(v.cfg.Name)
		if source == "" {
			source = normalizeViewShortcutSource(v.id)
		}
		if source == "" {
			source = normalizeViewShortcutSource(v.cfg.Key)
		}
		sources[i] = source
	}

	used := make(map[string]bool, len(m.views))
	shortcuts := make([]viewShortcut, 0, len(m.views))
	for i, v := range m.views {
		key := uniqueViewShortcut(sources, i)
		if key == "" {
			key = "v"
		}
		if used[key] {
			base := key
			for suffix := 2; used[key]; suffix++ {
				key = fmt.Sprintf("%s%d", base, suffix)
			}
		}
		used[key] = true
		shortcuts = append(shortcuts, viewShortcut{
			Key:   key,
			Label: v.cfg.Name,
			Index: i,
		})
	}
	return shortcuts
}

func normalizeViewShortcutSource(s string) string {
	var b strings.Builder
	for _, r := range strings.ToLower(s) {
		if unicode.IsLetter(r) || unicode.IsDigit(r) {
			b.WriteRune(r)
		}
	}
	return b.String()
}

func uniqueViewShortcut(sources []string, idx int) string {
	source := sources[idx]
	if source == "" {
		return ""
	}
	for length := 1; length <= len([]rune(source)); length++ {
		prefix := string([]rune(source)[:length])
		unique := true
		for j, other := range sources {
			if j == idx || other == "" {
				continue
			}
			if strings.HasPrefix(other, prefix) {
				unique = false
				break
			}
		}
		if unique {
			return prefix
		}
	}
	return source
}

func (m Model) handleActionPickerKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "esc":
		m.mode = modeNormal
		m.actionInput.Blur()
		m.actionInput.SetValue("")
		m.actionCursor = 0
		return m, nil
	case "up", "k":
		entries := m.filteredActionEntries()
		if len(entries) > 0 && m.actionCursor > 0 {
			m.actionCursor--
		}
		return m, nil
	case "down", "j":
		entries := m.filteredActionEntries()
		if len(entries) > 0 && m.actionCursor < len(entries)-1 {
			m.actionCursor++
		}
		return m, nil
	case "enter":
		entries := m.filteredActionEntries()
		if len(entries) == 0 {
			return m, nil
		}
		if m.actionCursor >= len(entries) {
			m.actionCursor = len(entries) - 1
		}
		entry := entries[m.actionCursor]
		m.mode = modeNormal
		m.actionInput.Blur()
		m.actionInput.SetValue("")
		m.actionCursor = 0
		return m.executeActionEntry(entry)
	default:
		var cmd tea.Cmd
		m.actionInput, cmd = m.actionInput.Update(msg)
		m.actionCursor = 0
		return m, cmd
	}
}

func (m Model) handleHardcoverQueryKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "enter":
		query := strings.TrimSpace(m.hardcoverInput.Value())
		if query == "" {
			m.mode = modeNormal
			m.hardcoverInput.Blur()
			return m, nil
		}
		m.mode = modeNormal
		m.hardcoverInput.Blur()
		m.status = "Searching Hardcover..."
		return m, searchHardcoverCmd(query)
	case "esc":
		m.mode = modeNormal
		m.hardcoverInput.Blur()
		m.hardcoverInput.SetValue("")
		return m, nil
	default:
		var cmd tea.Cmd
		m.hardcoverInput, cmd = m.hardcoverInput.Update(msg)
		return m, cmd
	}
}

func (m Model) handleHardcoverResultsKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "esc":
		m.mode = modeNormal
		m.hardcoverResults = nil
		m.status = "Hardcover search cancelled"
		return m, nil
	default:
		key := msg.String()
		if len(key) != 1 || key[0] < '1' || key[0] > '5' {
			return m, nil
		}
		idx := int(key[0] - '1')
		if idx < 0 || idx >= len(m.hardcoverResults) {
			return m, nil
		}
		book := m.hardcoverResults[idx]
		m.mode = modeNormal
		m.hardcoverResults = nil
		return m.createHardcoverBook(book)
	}
}

func (m Model) handleSyncOutboxKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	entries := m.syncOutboxEntries()
	switch msg.String() {
	case "esc", "q":
		if m.syncOutboxDetail {
			m.syncOutboxDetail = false
			return m, nil
		}
		m.mode = modeNormal
		return m, nil
	case "up", "k":
		if m.syncOutboxCursor > 0 {
			m.syncOutboxCursor--
		}
	case "down", "j":
		if m.syncOutboxCursor < len(entries)-1 {
			m.syncOutboxCursor++
		}
	case "pgup", "ctrl+u":
		m.syncOutboxCursor -= m.syncOutboxPageSize()
		if m.syncOutboxCursor < 0 {
			m.syncOutboxCursor = 0
		}
	case "pgdown", "ctrl+d":
		m.syncOutboxCursor += m.syncOutboxPageSize()
		if m.syncOutboxCursor >= len(entries) {
			m.syncOutboxCursor = len(entries) - 1
		}
	case "g":
		m.syncOutboxCursor = 0
	case "G":
		m.syncOutboxCursor = len(entries) - 1
	case "f", "tab":
		m.syncOutboxFilter = nextOutboxFilter(m.syncOutboxFilter)
		m.syncOutboxCursor = 0
		m.syncOutboxOffset = 0
	case "enter":
		if len(entries) > 0 {
			m.syncOutboxDetail = !m.syncOutboxDetail
		}
	case "r":
		if len(entries) > 0 && m.cache != nil {
			_ = m.cache.RetryOutbox(entries[m.syncOutboxCursor].ID)
			m.status = fmt.Sprintf("Queued retry for outbox #%d", entries[m.syncOutboxCursor].ID)
			return m, reconcileSyncCmd(m.baseDir, m.cache, m.appCfg)
		}
	case "R":
		if m.cache != nil {
			n, err := m.cache.RetryFailedOutbox()
			if err != nil {
				m.status = "Retry failed: " + err.Error()
				return m, nil
			}
			m.status = fmt.Sprintf("Queued retry for %d failed outbox entries", n)
			return m, reconcileSyncCmd(m.baseDir, m.cache, m.appCfg)
		}
	}
	if m.syncOutboxCursor < 0 {
		m.syncOutboxCursor = 0
	}
	if m.syncOutboxCursor >= len(entries) {
		m.syncOutboxCursor = len(entries) - 1
	}
	m.clampSyncOutboxOffset()
	return m, nil
}

func nextOutboxFilter(current string) string {
	filters := []string{"", "pending", "failed", "synced"}
	for i, f := range filters {
		if current == f {
			return filters[(i+1)%len(filters)]
		}
	}
	return ""
}

func (m Model) syncOutboxPageSize() int {
	n := m.height - 4
	if n < 1 {
		return 1
	}
	return n
}

func (m *Model) clampSyncOutboxOffset() {
	page := m.syncOutboxPageSize()
	if m.syncOutboxCursor < m.syncOutboxOffset {
		m.syncOutboxOffset = m.syncOutboxCursor
	}
	if m.syncOutboxCursor >= m.syncOutboxOffset+page {
		m.syncOutboxOffset = m.syncOutboxCursor - page + 1
	}
	if m.syncOutboxOffset < 0 {
		m.syncOutboxOffset = 0
	}
}

func (m Model) syncOutboxEntries() []cache.OutboxEntry {
	if m.cache == nil {
		return nil
	}
	entries, err := m.cache.OutboxEntriesByStatus(m.syncOutboxFilter, 500)
	if err != nil {
		return nil
	}
	return entries
}

func (m Model) actionEntries() []actionEntry {
	names := make([]string, 0, len(m.actions))
	for name := range m.actions {
		names = append(names, name)
	}
	sort.Strings(names)

	item, hasItem := m.selectedItem()
	entries := make([]actionEntry, 0, len(names)+5)
	for _, name := range names {
		act := m.actions[name]
		enabled := false
		if hasItem {
			enabled = act.MatchNote(config.Note{ID: item.ID, Path: item.Path, Type: item.Type, Tags: item.Tags, Frontmatter: item.Frontmatter, Body: item.Body})
		}
		entries = append(entries, actionEntry{
			Kind:        actionEntryConfigured,
			Name:        name,
			Description: act.Description,
			Filter:      "",
			Enabled:     enabled,
		})
	}

	entries = append(entries, actionEntry{
		Kind:        actionEntryGoodreads,
		Name:        "goodreads-import",
		Description: "Import from Goodreads",
		Enabled:     true,
	})
	entries = append(entries, actionEntry{
		Kind:        actionEntryHardcover,
		Name:        "hardcover-search",
		Description: "Search Hardcover",
		Enabled:     true,
	})
	entries = append(entries, actionEntry{
		Kind:        actionEntryURL,
		Name:        "url-to-note",
		Description: "Import URL as note",
		Enabled:     true,
	})
	entries = append(entries, actionEntry{Kind: actionEntryAttachImage, Name: "attach-image", Description: "Attach an image to the selected note", Enabled: hasItem})
	if m.appCfg != nil && m.appCfg.Sync.ServerURL != "" {
		entries = append(entries, actionEntry{
			Kind:        actionEntryStartSync,
			Name:        "start-sync",
			Description: "Start or retry sync with the configured server",
			Enabled:     true,
		})
		entries = append(entries, actionEntry{
			Kind:        actionEntrySyncOutbox,
			Name:        "sync-outbox",
			Description: "View local sync outbox and history",
			Enabled:     true,
		})
	}
	sort.SliceStable(entries, func(i, j int) bool {
		if entries[i].Enabled != entries[j].Enabled {
			return entries[i].Enabled
		}
		return entries[i].Name < entries[j].Name
	})
	return entries
}

func (m Model) filteredActionEntries() []actionEntry {
	query := strings.TrimSpace(m.actionInput.Value())
	entries := m.actionEntries()
	if query == "" {
		return entries
	}

	filtered := make([]actionEntry, 0, len(entries))
	for _, entry := range entries {
		haystack := entry.Name + " " + entry.Description
		if fuzzyMatch(query, haystack) {
			filtered = append(filtered, entry)
		}
	}
	return filtered
}

func (m Model) executeActionEntry(entry actionEntry) (tea.Model, tea.Cmd) {
	if !entry.Enabled {
		m.status = "Action is not applicable to this item"
		return m, nil
	}

	switch entry.Kind {
	case actionEntryConfigured:
		return m.applyAction(entry.Name)
	case actionEntryGoodreads:
		m.mode = modeImportURL
		m.importInput.SetValue("")
		m.importInput.Focus()
		return m, textinput.Blink
	case actionEntryHardcover:
		m.mode = modeHardcoverQuery
		m.hardcoverInput.SetValue("")
		m.hardcoverInput.Focus()
		return m, textinput.Blink
	case actionEntryURL:
		m.mode = modeURLImport
		m.urlInput.SetValue("")
		m.urlInput.Focus()
		return m, textinput.Blink
	case actionEntryStartSync:
		m.syncStatus = "syncing"
		m.status = "Starting sync..."
		return m, startSyncCmd(m.baseDir, m.cache, m.appCfg)
	case actionEntrySyncOutbox:
		m.mode = modeSyncOutbox
		return m, nil
	case actionEntryAttachImage:
		m.mode = modeAttachImage
		m.attachInput.SetValue("")
		m.attachInput.Focus()
		return m, textinput.Blink
	default:
		m.status = "Action not found"
		return m, nil
	}
}

func (m Model) selectedItem() (scanner.Item, bool) {
	items := m.currentItems()
	vs := m.currentView()
	if vs == nil || len(items) == 0 || vs.cursor >= len(items) {
		return scanner.Item{}, false
	}
	return items[vs.cursor], true
}

func (m Model) handleAttachImageKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "esc":
		m.mode = modeNormal
		m.attachInput.Blur()
		return m, nil
	case "enter":
		source := strings.TrimSpace(m.attachInput.Value())
		m.attachInput.Blur()
		m.mode = modeNormal
		if source == "" {
			return m, nil
		}
		item, ok := m.selectedItem()
		if !ok {
			m.status = "No item selected"
			return m, nil
		}
		file, err := os.Open(source)
		if err != nil {
			m.status = fmt.Sprintf("Open image: %v", err)
			return m, nil
		}
		asset, err := assets.Import(m.baseDir, filepath.Base(source), file)
		_ = file.Close()
		if err != nil {
			m.status = fmt.Sprintf("Attach image: %v", err)
			return m, nil
		}
		item.Body = strings.TrimRight(item.Body, "\n") + "\n\n![" + filepath.Base(asset.Path) + "](" + asset.Path + ")\n"
		if err := markdown.WriteFrontmatter(item.Path, item.Frontmatter, item.Body); err != nil {
			m.status = fmt.Sprintf("Save note: %v", err)
			return m, nil
		}
		if err := m.cache.NotifyWrite(item.Path); err != nil {
			m.status = fmt.Sprintf("Update cache: %v", err)
			return m, nil
		}
		if err := m.cache.Sync(); err != nil {
			m.status = fmt.Sprintf("Update assets: %v", err)
			return m, nil
		}
		m.status = "Attached " + asset.Path
		return m, tea.Batch(m.loadData(), m.reconcileIfStartedCmd())
	default:
		var cmd tea.Cmd
		m.attachInput, cmd = m.attachInput.Update(msg)
		return m, cmd
	}
}

func (m Model) canSearchHardcover() bool {
	vs := m.currentView()
	if vs == nil {
		return false
	}
	if isBookView(vs) {
		return true
	}
	items := m.currentItems()
	if len(items) == 0 || vs.cursor >= len(items) {
		return false
	}
	typ, _ := items[vs.cursor].Frontmatter["type"].(string)
	return typ == "book"
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
		m.actionError = err.Error()
		m.mode = modeActionError
		return m, nil
	} else {
		_ = m.cache.NotifyWrite(item.Path)
		m.status = fmt.Sprintf("Applied: %s", act.Description)
	}

	m.mode = modeNormal
	return m, tea.Batch(m.loadData(), m.reconcileIfStartedCmd())
}

func (m Model) handleActionErrorKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "esc", "enter", " ":
		m.mode = modeNormal
		m.actionError = ""
	}
	return m, nil
}

func (m Model) handleConfirmDeleteKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "y", "Y":
		m.deleteSelected()
		m.mode = modeNormal
		return m, tea.Batch(m.loadData(), m.reconcileIfStartedCmd())
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

func (m Model) handleURLImportKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "enter":
		rawURL := strings.TrimSpace(m.urlInput.Value())
		if rawURL == "" {
			m.mode = modeNormal
			m.urlInput.Blur()
			return m, nil
		}
		m.mode = modeNormal
		m.urlInput.Blur()
		m.status = "Importing URL..."
		return m, importURLCmd(m.baseDir, m.cache, rawURL)
	case "esc":
		m.mode = modeNormal
		m.urlInput.Blur()
		m.urlInput.SetValue("")
		return m, nil
	default:
		var cmd tea.Cmd
		m.urlInput, cmd = m.urlInput.Update(msg)
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
		m.viewMenuInput = ""
		return m, nil

	case ":":
		m.mode = modeActionPicker
		m.actionInput.SetValue("")
		m.actionInput.Focus()
		m.actionCursor = 0
		return m, textinput.Blink

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

	case "N":
		return m.createNew(true)

	case "E":
		return m.enableEncryptionSelected()

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
	m.viewMenuInput = ""
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
			matched, err := m.cfg.MatchView(vs.id, config.Note{ID: item.ID, Path: item.Path, Type: item.Type, Tags: item.Tags, Frontmatter: item.Frontmatter, Body: item.Body})
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
		return itemLess(vs.items[i], vs.items[j], sortField, desc)
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

func (m *Model) applySubviewFilter(vs *viewState) {
	if vs.activeSubview >= len(vs.cfg.Subviews) {
		vs.filteredItems = vs.items
		return
	}
	vs.filteredItems = nil
	for _, item := range vs.items {
		matched, err := m.cfg.MatchSubview(vs.id, vs.activeSubview, config.Note{ID: item.ID, Path: item.Path, Type: item.Type, Tags: item.Tags, Frontmatter: item.Frontmatter, Body: item.Body})
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
	if item.Frontmatter["encrypted"] == true {
		m.mode = modeEncryptedEdit
		m.encryptedEdit = &item
		m.attachInput.SetValue("")
		m.attachInput.Prompt = "Passphrase: "
		m.attachInput.EchoMode = textinput.EchoPassword
		m.attachInput.Focus()
		return m, textinput.Blink
	}
	if item.Path == "" {
		return m, nil
	}
	editor := os.Getenv("EDITOR")
	if editor == "" {
		editor = "vim"
	}
	relPath, err := filepath.Rel(m.baseDir, item.Path)
	if err != nil {
		relPath = item.Path
	}
	c := exec.Command(editor, relPath)
	c.Dir = m.baseDir
	return m, tea.ExecProcess(c, func(err error) tea.Msg {
		return refreshMsg{}
	})
}

// enableEncryptionSelected encrypts the selected note's body after prompting
// for a passphrase. Existing encrypted notes are intentionally left unchanged.
func (m Model) enableEncryptionSelected() (tea.Model, tea.Cmd) {
	items := m.currentItems()
	vs := m.currentView()
	if vs == nil || len(items) == 0 || vs.cursor >= len(items) {
		return m, nil
	}
	item := items[vs.cursor]
	if item.Frontmatter["encrypted"] == true || encryption.IsEncrypted(item.Body) {
		m.status = "Note is already encrypted"
		return m, nil
	}
	m.encryptedEdit = &item
	m.mode = modeEncryptedEdit
	m.attachInput.SetValue("")
	m.attachInput.Prompt = "New passphrase: "
	m.attachInput.EchoMode = textinput.EchoPassword
	m.attachInput.Focus()
	return m, textinput.Blink
}

func (m Model) handleEncryptedEditKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	if msg.String() == "esc" {
		m.mode = modeNormal
		m.encryptedEdit = nil
		m.attachInput.Blur()
		return m, nil
	}
	if msg.String() != "enter" {
		var cmd tea.Cmd
		m.attachInput, cmd = m.attachInput.Update(msg)
		return m, cmd
	}
	if m.encryptedEdit == nil {
		m.mode = modeNormal
		return m, nil
	}
	pass := m.attachInput.Value()
	if pass == "" {
		m.status = "A passphrase is required"
		return m, nil
	}
	if !encryption.IsEncrypted(m.encryptedEdit.Body) {
		body, err := encryption.Encrypt(m.encryptedEdit.ID, pass, m.encryptedEdit.Body)
		if err != nil {
			m.status = fmt.Sprintf("Encrypt note: %v", err)
			return m, nil
		}
		fm := make(map[string]interface{}, len(m.encryptedEdit.Frontmatter)+1)
		for key, value := range m.encryptedEdit.Frontmatter {
			fm[key] = value
		}
		fm["encrypted"] = true
		if err := markdown.WriteFrontmatter(m.encryptedEdit.Path, fm, body); err != nil {
			m.status = fmt.Sprintf("Save encrypted note: %v", err)
			return m, nil
		}
		m.mode = modeNormal
		m.attachInput.Blur()
		m.attachInput.EchoMode = textinput.EchoNormal
		m.encryptedEdit = nil
		m.status = "Note encrypted"
		return m, tea.Batch(m.loadData(), m.reconcileIfStartedCmd())
	}
	plain, err := encryption.Decrypt(m.encryptedEdit.ID, pass, m.encryptedEdit.Body)
	if err != nil {
		m.status = "Unable to decrypt note"
		return m, nil
	}
	return m.openEncryptedEditor(plain, pass)
}

func (m Model) handleCreateEncryptedKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	if msg.String() == "esc" {
		m.mode = modeNormal
		m.attachInput.Blur()
		m.pendingEncryptedContent = ""
		m.pendingEncryptedPath = ""
		m.createEncrypted = false
		return m, nil
	}
	if msg.String() != "enter" {
		var cmd tea.Cmd
		m.attachInput, cmd = m.attachInput.Update(msg)
		return m, cmd
	}
	pass := m.attachInput.Value()
	if pass == "" {
		m.status = "A passphrase is required"
		return m, nil
	}
	fm, body, err := markdown.ParseFrontmatterBytes([]byte(m.pendingEncryptedContent))
	if err != nil {
		m.status = fmt.Sprintf("Parse new note: %v", err)
		return m, nil
	}
	noteID := markdown.FMString(fm, "id")
	if noteID == "" {
		m.status = "Encrypted notes require an id"
		return m, nil
	}
	fm["encrypted"] = true
	ciphertext, err := encryption.Encrypt(noteID, pass, body)
	if err != nil {
		m.status = fmt.Sprintf("Encrypt new note: %v", err)
		return m, nil
	}
	if err := os.MkdirAll(filepath.Dir(m.pendingEncryptedPath), 0755); err != nil {
		m.status = fmt.Sprintf("Create encrypted note directory: %v", err)
		return m, nil
	}
	if _, err := os.Stat(m.pendingEncryptedPath); err == nil {
		m.status = "An item with this id already exists"
		return m, nil
	}
	if err := markdown.WriteFrontmatter(m.pendingEncryptedPath, fm, ciphertext); err != nil {
		m.status = fmt.Sprintf("Save encrypted note: %v", err)
		return m, nil
	}
	m.encryptedEdit = &scanner.Item{Path: m.pendingEncryptedPath, Frontmatter: fm, Body: ciphertext}
	m.pendingEncryptedContent = ""
	m.pendingEncryptedPath = ""
	m.createEncrypted = false
	return m.openEncryptedEditor(body, pass)
}

// openEncryptedEditor writes the complete plaintext note to a private temporary
// file, so users can edit frontmatter as well as the body.
func (m Model) openEncryptedEditor(body, pass string) (tea.Model, tea.Cmd) {
	f, err := os.CreateTemp("", "exokephalos-encrypted-*.md")
	if err != nil {
		m.status = fmt.Sprintf("Create temp edit: %v", err)
		return m, nil
	}
	_ = f.Chmod(0600)
	if err := markdown.WriteFrontmatter(f.Name(), m.encryptedEdit.Frontmatter, body); err != nil {
		_ = f.Close()
		_ = os.Remove(f.Name())
		m.status = fmt.Sprintf("Write temp edit: %v", err)
		return m, nil
	}
	_ = f.Close()
	m.encryptedTemp, m.encryptedPass, m.mode = f.Name(), pass, modeNormal
	m.attachInput.Blur()
	m.attachInput.EchoMode = textinput.EchoNormal
	editor := os.Getenv("EDITOR")
	if editor == "" {
		editor = "vim"
	}
	c := exec.Command(editor, f.Name())
	return m, tea.ExecProcess(c, func(err error) tea.Msg { return encryptedEditMsg{err} })
}

func (m Model) importBook(url string) (tea.Model, tea.Cmd) {
	meta, err := goodreads.FetchBook(url)
	if err != nil {
		m.status = fmt.Sprintf("Import error: %v", err)
		return m, nil
	}

	for _, vs := range m.views {
		if isBookView(&vs) {
			item, err := itemcreate.New(m.baseDir, "book", meta.Title, "")
			if err != nil {
				m.status = fmt.Sprintf("Create error: %v", err)
				return m, nil
			}
			item.Frontmatter["tags"] = toInterfaceSlice([]string{"to-read"})
			item.Frontmatter["author"] = toInterfaceSlice(meta.Author)
			item.Frontmatter["url"] = meta.URL
			item.Frontmatter["pages"] = meta.Pages
			item.Frontmatter["cover"] = meta.Cover
			if err := itemcreate.Write(item); err != nil {
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

func searchHardcoverCmd(query string) tea.Cmd {
	return func() tea.Msg {
		client := hardcover.NewClient(os.Getenv("HARDCOVER_TOKEN"))
		results, err := client.Search(query, 5)
		return hardcoverSearchMsg{results: results, err: err}
	}
}

func importURLCmd(baseDir string, c *cache.Cache, rawURL string) tea.Cmd {
	return func() tea.Msg {
		r := repo.New(baseDir, c)
		result, err := urlimport.Import(context.Background(), r, baseDir, rawURL)
		return urlImportMsg{result: result, err: err}
	}
}

func (m Model) createHardcoverBook(book hardcover.Book) (tea.Model, tea.Cmd) {
	for _, vs := range m.views {
		if isBookView(&vs) {
			title := hardcoverBookTitle(book)
			item, err := itemcreate.New(m.baseDir, "book", title, strings.TrimSpace(book.Description))
			if err != nil {
				m.status = fmt.Sprintf("Create error: %v", err)
				return m, nil
			}
			item.Frontmatter["tags"] = toInterfaceSlice([]string{"to-read"})
			item.Frontmatter["author"] = toInterfaceSlice(book.Authors)
			item.Frontmatter["url"] = book.URL
			item.Frontmatter["pages"] = book.Pages
			item.Frontmatter["cover"] = book.Cover
			if book.ISBN != "" {
				item.Frontmatter["isbn"] = book.ISBN
			}
			if err := itemcreate.Write(item); err != nil {
				m.status = fmt.Sprintf("Write error: %v", err)
				return m, nil
			}
			m.status = fmt.Sprintf("Added book: %s", title)
			return m, tea.Batch(m.loadData(), m.reconcileIfStartedCmd())
		}
	}

	m.status = "No books view configured"
	return m, nil
}

func toInterfaceSlice(values []string) []interface{} {
	result := make([]interface{}, len(values))
	for i, value := range values {
		result[i] = value
	}
	return result
}

func hardcoverBookTitle(book hardcover.Book) string {
	if strings.TrimSpace(book.Series) == "" {
		return book.Title
	}
	return fmt.Sprintf("%s (%s)", book.Title, book.Series)
}

// appendImportedDescription adds an imported book description to the body
// without requiring it to be present in the user's books template.
func appendImportedDescription(content, description string) string {
	description = strings.TrimSpace(description)
	if description == "" {
		return content
	}

	content = strings.TrimRight(content, "\n")
	if content == "" {
		return description + "\n"
	}
	return content + "\n\n" + description + "\n"
}

func isBookView(vs *viewState) bool {
	if vs == nil {
		return false
	}
	return strings.EqualFold(strings.TrimSuffix(vs.id, "s"), "book") ||
		strings.Contains(strings.ToLower(vs.cfg.Name), "book")
}

func fuzzyMatch(query, value string) bool {
	query = strings.ToLower(strings.TrimSpace(query))
	value = strings.ToLower(value)
	if query == "" {
		return true
	}
	if strings.Contains(value, query) {
		return true
	}

	j := 0
	for i := 0; i < len(value) && j < len(query); i++ {
		if value[i] == query[j] {
			j++
		}
	}
	return j == len(query)
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

func (m *Model) reloadConfig() error {
	cfg, err := config.Load(m.baseDir)
	if err != nil {
		return err
	}
	m.cfg = cfg

	orderedViews := m.cfg.OrderedViews()
	views := make([]viewState, 0, len(orderedViews))

	for _, ov := range orderedViews {
		vs := viewState{
			cfg:            ov.Config,
			id:             ov.ID,
			tagCountsCache: &tagCountsCache{},
		}

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

	actions := make(map[string]*action.Action)
	for name, ac := range m.cfg.Actions {
		act, err := action.Compile(name, ac)
		if err != nil {
			continue
		}
		actions[name] = act
	}

	// Try to preserve active view and its cursor/scroll position if it still exists
	oldActiveViewID := ""
	var oldViewState *viewState
	if m.activeView >= 0 && m.activeView < len(m.views) {
		oldActiveViewID = m.views[m.activeView].id
		oldViewState = &m.views[m.activeView]
	}

	m.views = views
	m.actions = actions

	m.activeView = m.cfg.DefaultViewIndex()
	if oldActiveViewID != "" {
		for i, v := range m.views {
			if v.id == oldActiveViewID {
				m.activeView = i
				// Restore cursor, offset, selected tags, etc.
				if oldViewState != nil {
					m.views[i].cursor = oldViewState.cursor
					m.views[i].offset = oldViewState.offset
					m.views[i].selectedTags = oldViewState.selectedTags
					m.views[i].tagCursor = oldViewState.tagCursor
					m.views[i].tagOffset = oldViewState.tagOffset
					m.views[i].tagFilterValue = oldViewState.tagFilterValue
					m.views[i].textFilter = oldViewState.textFilter
				}
				break
			}
		}
	}

	m.invalidateAllTagCounts()
	return nil
}

// --- Data loading ---

func (m Model) loadData() tea.Cmd {
	c := m.cache
	return func() tea.Msg {
		if c == nil {
			return dataLoadedMsg{allItems: nil}
		}
		if err := c.Sync(); err != nil {
			return dataLoadedMsg{allItems: nil}
		}
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
