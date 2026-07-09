package tui

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
	"github.com/gnur/exokephalos/internal/scanner"
)

// --- View rendering ---

func (m Model) View() string {
	if !m.ready {
		return "Loading..."
	}

	if len(m.views) == 0 {
		return "No views configured. Check your .exo.toml file."
	}

	// Render popup overlays
	if m.mode == modeViewMenu {
		return m.renderViewMenuOverlay()
	}
	if m.mode == modeActionPicker {
		return m.renderActionPickerOverlay()
	}
	if m.mode == modeHardcoverResults {
		return m.renderHardcoverResultsOverlay()
	}

	header := m.renderHeader()
	body := m.renderBody()
	footer := m.renderFooter()

	return lipgloss.JoinVertical(lipgloss.Left, header, body, footer)
}

func (m Model) renderViewMenuOverlay() string {
	entries := make([]popupEntry, 0, len(m.views))
	for _, shortcut := range m.viewShortcuts() {
		entries = append(entries, popupEntry{
			Key:   shortcut.Key,
			Label: shortcut.Label,
		})
	}
	title := "Go to view"
	if m.viewMenuInput != "" {
		title += ": " + m.viewMenuInput
	}
	return renderPopup(title, entries, m.width, m.height)
}

func (m Model) renderActionPickerOverlay() string {
	entries := m.filteredActionEntries()
	lines := []string{
		popupTitleStyle.Render("Actions"),
		searchPromptStyle.Render(m.actionInput.View()),
		"",
	}

	if len(entries) == 0 {
		lines = append(lines, popupHintStyle.Render("  no matching actions"))
	} else {
		for i, entry := range entries {
			label := fmt.Sprintf("%s — %s", entry.Name, entry.Description)
			if !entry.Enabled && entry.Filter != "" {
				label += "  requires: " + entry.Filter
			}
			prefix := "  "
			if i == m.actionCursor {
				prefix = "> "
			}
			style := popupLabelStyle
			if !entry.Enabled {
				style = disabledActionStyle
			} else if i == m.actionCursor {
				style = selectedItemStyle
			}
			lines = append(lines, style.Render(prefix+label))
		}
	}

	lines = append(lines, "", popupHintStyle.Render("  enter to run  esc to cancel"))
	return renderFloating(strings.Join(lines, "\n"), m.width, m.height, actionPickerWidth(entries))
}

func actionPickerWidth(entries []actionEntry) int {
	width := 44
	for _, entry := range entries {
		labelWidth := len(entry.Name) + len(entry.Description) + 5
		if !entry.Enabled && entry.Filter != "" {
			labelWidth += len(entry.Filter) + len("  requires: ")
		}
		if labelWidth > width {
			width = labelWidth
		}
	}
	return width
}

func (m Model) renderHardcoverResultsOverlay() string {
	entries := make([]popupEntry, 0, len(m.hardcoverResults))
	for i, book := range m.hardcoverResults {
		label := book.Title
		if len(book.Authors) > 0 {
			label += " — " + strings.Join(book.Authors, ", ")
		}
		if book.Year != "" {
			label += " (" + book.Year + ")"
		}
		entries = append(entries, popupEntry{
			Key:   fmt.Sprintf("%d", i+1),
			Label: label,
		})
	}
	return renderPopup("Hardcover results", entries, m.width, m.height)
}

func (m Model) renderHeader() string {
	vs := m.currentView()
	if vs == nil {
		return ""
	}

	// View name + subview tabs
	viewLabel := activeTabStyle.Render(vs.cfg.Name)

	// Subview tabs
	var subParts []string
	for i, sv := range vs.cfg.Subviews {
		label := fmt.Sprintf("[%s]", sv.Name)
		if i == vs.activeSubview {
			subParts = append(subParts, activeTabStyle.Render(label))
		} else {
			subParts = append(subParts, inactiveTabStyle.Render(label))
		}
	}

	subLine := ""
	if len(subParts) > 1 {
		subLine = lipgloss.JoinHorizontal(lipgloss.Top, subParts...)
	}

	headerContent := viewLabel
	if subLine != "" {
		headerContent += "  " + subLine
	}

	return headerStyle.Width(m.width).Render(headerContent)
}

func (m Model) renderBody() string {
	if m.currentViewShowsTags() {
		return m.renderThreeColumnBody()
	}
	return m.renderTwoColumnBody()
}

func (m Model) renderThreeColumnBody() string {
	vs := m.currentView()
	if vs == nil {
		return ""
	}

	tagW := m.tagPaneWidth()
	listW := m.listWidth()
	listH := m.contentHeight() - 1

	// --- Filter headers ---
	tagHeader := ""
	if vs.tagFilterValue != "" {
		tagHeader = searchPromptStyle.Render(truncate("/ "+vs.tagFilterValue, tagW))
	} else {
		tagHeader = lipgloss.NewStyle().Foreground(mutedColor).Render("Tags")
	}
	listHeader := ""
	if vs.textFilter != "" {
		listHeader = searchPromptStyle.Render(truncate("/ "+vs.textFilter, listW))
	} else {
		listHeader = lipgloss.NewStyle().Foreground(mutedColor).Render(vs.cfg.Name)
	}
	previewHeader := lipgloss.NewStyle().Foreground(mutedColor).Render("Preview")

	// --- Tag pane ---
	tagCounts := m.computeTagCounts()
	var tagLines []string
	tagEnd := vs.tagOffset + listH
	if tagEnd > len(tagCounts) {
		tagEnd = len(tagCounts)
	}
	for i := vs.tagOffset; i < tagEnd; i++ {
		tc := tagCounts[i]
		selected := false
		for _, st := range vs.selectedTags {
			if st == tc.tag {
				selected = true
				break
			}
		}
		indicator := "[ ]"
		if selected {
			indicator = "[x]"
		}
		label := fmt.Sprintf("%s %s (%d)", indicator, truncate(tc.tag, tagW-10), tc.count)
		if i == vs.tagCursor && m.pane == paneTags {
			label = selectedItemStyle.Render(label)
		} else if selected {
			label = lipgloss.NewStyle().Foreground(successColor).Render(label)
		} else {
			label = normalItemStyle.Render(label)
		}
		tagLines = append(tagLines, label)
	}
	for len(tagLines) < listH {
		tagLines = append(tagLines, "")
	}

	tagBorder := lipgloss.NormalBorder()
	tagStyle := lipgloss.NewStyle().Width(tagW).Height(listH).
		BorderStyle(tagBorder).BorderRight(true).BorderForeground(mutedColor)
	if m.pane == paneTags {
		tagStyle = tagStyle.BorderForeground(primaryColor)
	}
	tagColumn := lipgloss.JoinVertical(lipgloss.Left,
		lipgloss.NewStyle().Width(tagW).Render(tagHeader),
		tagStyle.Render(strings.Join(tagLines, "\n")),
	)

	// --- List pane ---
	items := m.currentItems()
	listLines := m.renderItemLines(items, vs, listW, listH, m.pane == paneList)
	for len(listLines) < listH {
		listLines = append(listLines, "")
	}

	listBorder := lipgloss.NewStyle().Width(listW).Height(listH).
		BorderStyle(lipgloss.NormalBorder()).BorderRight(true).BorderForeground(mutedColor)
	if m.pane == paneList {
		listBorder = listBorder.BorderForeground(primaryColor)
	}
	listColumn := lipgloss.JoinVertical(lipgloss.Left,
		lipgloss.NewStyle().Width(listW).Render(listHeader),
		listBorder.Render(strings.Join(listLines, "\n")),
	)

	// --- Preview pane ---
	m.preview.Width = m.previewWidth()
	m.preview.Height = listH
	previewStyle := lipgloss.NewStyle().Width(m.previewWidth()).Height(listH)
	if m.pane == panePreview {
		previewStyle = previewStyle.BorderStyle(lipgloss.NormalBorder()).BorderLeft(true).BorderForeground(primaryColor)
	}
	previewColumn := lipgloss.JoinVertical(lipgloss.Left,
		lipgloss.NewStyle().Width(m.previewWidth()).Render(previewHeader),
		previewStyle.Render(m.preview.View()),
	)

	return lipgloss.JoinHorizontal(lipgloss.Top, tagColumn, listColumn, previewColumn)
}

func (m Model) renderTwoColumnBody() string {
	vs := m.currentView()
	if vs == nil {
		return ""
	}

	items := m.currentItems()
	listW := m.listWidth()
	listH := m.contentHeight() - 1

	// Filter header
	listHeader := ""
	if vs.textFilter != "" {
		listHeader = searchPromptStyle.Render(truncate("/ "+vs.textFilter, listW))
	} else {
		listHeader = lipgloss.NewStyle().Foreground(mutedColor).Render(vs.cfg.Name)
	}

	// Render list
	listLines := m.renderItemLines(items, vs, listW, listH, true)
	for len(listLines) < listH {
		listLines = append(listLines, "")
	}

	listColumn := lipgloss.JoinVertical(lipgloss.Left,
		lipgloss.NewStyle().Width(listW).Render(listHeader),
		lipgloss.NewStyle().Width(listW).Height(listH).Render(strings.Join(listLines, "\n")),
	)

	// Preview panel
	m.preview.Width = m.previewWidth()
	m.preview.Height = listH
	previewContent := previewBorderStyle.Height(listH).Render(m.preview.View())

	return lipgloss.JoinHorizontal(lipgloss.Top, listColumn, previewContent)
}

func (m Model) renderFooter() string {
	if m.mode == modeConfirmDelete {
		return confirmStyle.Render(m.status)
	}
	if m.mode == modeCreatePrompt {
		return searchPromptStyle.Render(m.promptInput.View())
	}
	if m.mode == modeImportURL {
		return searchPromptStyle.Render(m.importInput.View())
	}
	if m.mode == modeHardcoverQuery {
		return searchPromptStyle.Render(m.hardcoverInput.View())
	}
	if m.mode == modeActionPicker {
		return searchPromptStyle.Render(m.actionInput.View())
	}
	if m.mode == modeSearchTags {
		return searchPromptStyle.Render(m.tagFilterInput.View())
	}
	if m.mode == modeSearchItems {
		return searchPromptStyle.Render(m.textFilterInput.View())
	}

	var hint string
	if m.currentViewShowsTags() {
		hint = " /:filter  ::actions  space:tag  h/l:pane  tab:subview  g:views  n:new  e:edit  d:del  q:quit"
	} else {
		hint = " /:filter  ::actions  tab:subview  g:views  n:new  e:edit  d:del  q:quit"
	}

	// Show selected tags
	vs := m.currentView()
	if vs != nil && len(vs.selectedTags) > 0 {
		tagInfo := fmt.Sprintf(" [tags: %s]", strings.Join(vs.selectedTags, ", "))
		hint = lipgloss.NewStyle().Foreground(successColor).Render(tagInfo) + hint
	}

	if m.status != "" {
		hint = m.status + "  " + hint
	}
	return footerStyle.Width(m.width).Render(hint)
}

func truncate(s string, max int) string {
	if max <= 0 {
		return ""
	}
	if len(s) <= max {
		return s
	}
	return s[:max-1] + "…"
}

// renderItemLines renders list items with year dividers.
// It handles offset, cursor highlighting, subtitles, and year headers.
func (m Model) renderItemLines(items []scanner.Item, vs *viewState, listW, listH int, showCursor bool) []string {
	var lines []string
	prevYear := ""

	// Determine the year of the item at offset to know if we need an initial divider
	if vs.offset > 0 && vs.offset < len(items) {
		prevYear = items[vs.offset].Year(vs.cfg.SortField)
	}

	for i := vs.offset; i < len(items) && len(lines) < listH; i++ {
		item := items[i]
		year := item.Year(vs.cfg.SortField)
		if year == "" {
			year = "Unknown"
		}

		// Insert year divider when year changes
		if year != prevYear {
			if len(lines) >= listH {
				break
			}
			divider := yearDividerStyle.Render(padDivider(year, listW))
			lines = append(lines, divider)
			prevYear = year
			if len(lines) >= listH {
				break
			}
		}

		// Render item title
		title := truncate(item.Title(vs.cfg.TitleField), listW-4)
		if i == vs.cursor && showCursor {
			title = selectedItemStyle.Render("> " + title)
		} else if i == vs.cursor {
			title = lipgloss.NewStyle().Foreground(lipgloss.Color("#FFFFFF")).Bold(true).Render("  " + title)
		} else {
			title = normalItemStyle.Render("  " + title)
		}
		lines = append(lines, title)

		// Subtitle
		if vs.cfg.SubtitleField != "" && len(lines) < listH {
			sub := item.Subtitle(vs.cfg.SubtitleField)
			if sub != "" {
				subLine := "    " + truncate(sub, listW-6)
				lines = append(lines, lipgloss.NewStyle().Foreground(mutedColor).Render(subLine))
			}
		}
	}
	return lines
}

// padDivider creates a year divider line like "── 2024 ──────"
func padDivider(year string, width int) string {
	label := "── " + year + " "
	remaining := width - len(label)
	if remaining > 0 {
		label += strings.Repeat("─", remaining)
	}
	return label
}
