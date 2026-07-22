package tui

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"

	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/gnur/exokephalos/internal/itemcreate"
	"gopkg.in/yaml.v3"
)

// createNew starts the common creation flow. Views filter and present items;
// they do not determine an item's shape or type.
func (m Model) createNew(encrypt ...bool) (tea.Model, tea.Cmd) {
	m.createEncrypted = len(encrypt) > 0 && encrypt[0]
	m.createVars = map[string]string{"Type": "note"}
	m.pendingPrompts = []string{"Type", "Title"}
	m.currentPromptIdx = 0
	m.mode = modeCreatePrompt
	m.promptInput.SetValue("note")
	m.promptInput.Prompt = "Type: "
	m.promptInput.Focus()
	return m, textinput.Blink
}

// finishCreate creates a standard item and opens it for body editing.
func (m Model) finishCreate() (tea.Model, tea.Cmd) {
	item, err := itemcreate.New(m.baseDir, m.createVars["Type"], m.createVars["Title"], "")
	if err != nil {
		m.status = fmt.Sprintf("Create error: %v", err)
		return m, nil
	}
	if m.createEncrypted {
		content, err := renderItem(item)
		if err != nil {
			m.status = fmt.Sprintf("Create error: %v", err)
			return m, nil
		}
		return m.prepareEncryptedCreate(content, item.Path)
	}
	if err := itemcreate.Write(item); err != nil {
		m.status = fmt.Sprintf("Write error: %v", err)
		return m, nil
	}
	return m.openCreatedItem(item.Path)
}

func (m Model) openCreatedItem(path string) (tea.Model, tea.Cmd) {
	editor := os.Getenv("EDITOR")
	if editor == "" {
		editor = "vim"
	}
	return m, tea.ExecProcess(exec.Command(editor, path), func(error) tea.Msg { return refreshMsg{} })
}

// prepareEncryptedCreate asks for the passphrase before putting the new note on
// disk, keeping its initial plaintext out of the repository.
func (m Model) prepareEncryptedCreate(content, path string) (tea.Model, tea.Cmd) {
	m.pendingEncryptedContent = content
	m.pendingEncryptedPath = path
	m.mode = modeCreateEncrypted
	m.attachInput.SetValue("")
	m.attachInput.Prompt = "New passphrase: "
	m.attachInput.EchoMode = textinput.EchoPassword
	m.attachInput.Focus()
	return m, textinput.Blink
}

func renderItem(item itemcreate.Item) (string, error) {
	var content bytes.Buffer
	content.WriteString("---\n")
	encoder := yaml.NewEncoder(&content)
	if err := encoder.Encode(item.Frontmatter); err != nil {
		return "", err
	}
	if err := encoder.Close(); err != nil {
		return "", err
	}
	content.WriteString("---\n\n")
	content.WriteString(item.Body)
	return content.String(), nil
}
