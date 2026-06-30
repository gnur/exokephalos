package tui

import "github.com/charmbracelet/lipgloss"

var (
	// Colors
	primaryColor   = lipgloss.Color("#7C3AED")
	secondaryColor = lipgloss.Color("#06B6D4")
	mutedColor     = lipgloss.Color("#6B7280")
	dangerColor    = lipgloss.Color("#EF4444")
	successColor   = lipgloss.Color("#10B981")

	// Tab styles
	activeTabStyle = lipgloss.NewStyle().
			Bold(true).
			Foreground(primaryColor).
			Padding(0, 1)

	inactiveTabStyle = lipgloss.NewStyle().
				Foreground(mutedColor).
				Padding(0, 1)

	// Header/footer
	headerStyle = lipgloss.NewStyle().
			Bold(true).
			BorderStyle(lipgloss.NormalBorder()).
			BorderBottom(true).
			BorderForeground(mutedColor).
			Width(80)

	footerStyle = lipgloss.NewStyle().
			Foreground(mutedColor).
			BorderStyle(lipgloss.NormalBorder()).
			BorderTop(true).
			BorderForeground(mutedColor)

	// List styles
	selectedItemStyle = lipgloss.NewStyle().
				Foreground(primaryColor).
				Bold(true)

	normalItemStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#FFFFFF"))

	// Preview
	previewBorderStyle = lipgloss.NewStyle().
				BorderStyle(lipgloss.NormalBorder()).
				BorderLeft(true).
				BorderForeground(mutedColor).
				PaddingLeft(1)

	// Search
	searchPromptStyle = lipgloss.NewStyle().
				Foreground(secondaryColor).
				Bold(true)

	// Confirm dialog
	confirmStyle = lipgloss.NewStyle().
			Foreground(dangerColor).
			Bold(true)

	// Popup styles
	popupStyle = lipgloss.NewStyle().
			Border(lipgloss.RoundedBorder()).
			BorderForeground(primaryColor).
			Padding(1, 2)

	popupTitleStyle = lipgloss.NewStyle().
			Bold(true).
			Foreground(primaryColor)

	popupKeyStyle = lipgloss.NewStyle().
			Bold(true).
			Foreground(secondaryColor)

	popupLabelStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#FFFFFF"))

	popupHintStyle = lipgloss.NewStyle().
			Foreground(mutedColor).
			Italic(true)

	yearDividerStyle = lipgloss.NewStyle().
				Foreground(lipgloss.Color("#9CA3AF")).
				Bold(true)
)
