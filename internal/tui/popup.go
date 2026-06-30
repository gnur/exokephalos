package tui

import (
	"strings"

	"github.com/charmbracelet/lipgloss"
)

// popupEntry represents a single item in a popup menu.
type popupEntry struct {
	Key   string
	Label string
}

// renderPopup renders a floating popup menu with the given title and entries.
// The key prefix is highlighted in each entry.
func renderPopup(title string, entries []popupEntry, width, height int) string {
	// Build content lines
	var lines []string
	lines = append(lines, popupTitleStyle.Render(title))
	lines = append(lines, "")

	for _, e := range entries {
		keyPart := popupKeyStyle.Render(e.Key)
		labelPart := popupLabelStyle.Render(" " + e.Label)
		lines = append(lines, "  "+keyPart+labelPart)
	}

	lines = append(lines, "")
	lines = append(lines, popupHintStyle.Render("  esc to cancel"))

	content := strings.Join(lines, "\n")

	// Calculate popup dimensions
	popupW := 30
	for _, e := range entries {
		w := len(e.Key) + len(e.Label) + 6
		if w > popupW {
			popupW = w
		}
	}
	if len(title)+4 > popupW {
		popupW = len(title) + 4
	}

	popup := popupStyle.Width(popupW).Render(content)

	// Center the popup on screen
	popupH := strings.Count(popup, "\n") + 1
	padTop := (height - popupH) / 3
	padLeft := (width - popupW - 4) / 2

	if padTop < 0 {
		padTop = 0
	}
	if padLeft < 0 {
		padLeft = 0
	}

	// Build the positioned popup
	positioned := strings.Repeat("\n", padTop) +
		lipgloss.NewStyle().PaddingLeft(padLeft).Render(popup)

	return positioned
}
