package tui

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"text/template"
	"time"

	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/gnur/exokephalos/internal/id"
	"github.com/gnur/exokephalos/internal/markdown"
)

// autoFillVars are template variables that get filled automatically.
var autoFillVars = map[string]bool{
	"Date":     true,
	"DateTime": true,
	"ID":       true,
	"Year":     true,
	"Month":    true,
	"Day":      true,
	"Slug":     true,
}

// templateVarRegex matches Go template variables like {{.VarName}}.
var templateVarRegex = regexp.MustCompile(`\{\{\s*\.(\w+)\s*\}\}`)

// createNew starts the creation flow for the current view.
func (m Model) createNew(encrypt ...bool) (tea.Model, tea.Cmd) {
	m.createEncrypted = len(encrypt) > 0 && encrypt[0]
	vs := m.currentView()
	if vs == nil {
		return m, nil
	}

	// Find which template variables need prompting
	vars := templateVarRegex.FindAllStringSubmatch(vs.cfg.Template, -1)

	// Collect unique non-auto variables
	seen := make(map[string]bool)
	var prompts []string
	for _, match := range vars {
		varName := match[1]
		if autoFillVars[varName] || seen[varName] {
			continue
		}
		seen[varName] = true
		prompts = append(prompts, varName)
	}

	// Initialize create vars with auto-fill values
	createVars := newAutoFillVars()

	if len(prompts) == 0 {
		// No prompts needed - create directly
		m.createVars = createVars
		content, path, err := renderCreateTemplate(vs.cfg.Template, vs.id, m.baseDir, createVars)
		if err != nil {
			m.status = fmt.Sprintf("Template error: %v", err)
			return m, nil
		}
		if m.createEncrypted {
			return m.prepareEncryptedCreate(content, path)
		}
		if err := writeNewFile(path, content); err != nil {
			m.status = fmt.Sprintf("Write error: %v", err)
			return m, nil
		}
		// Open in editor
		editor := os.Getenv("EDITOR")
		if editor == "" {
			editor = "vim"
		}
		c := exec.Command(editor, path)
		return m, tea.ExecProcess(c, func(err error) tea.Msg {
			return refreshMsg{}
		})
	}

	// Start prompting
	m.createVars = createVars
	m.pendingPrompts = prompts
	m.currentPromptIdx = 0
	m.mode = modeCreatePrompt
	m.promptInput.SetValue("")
	m.promptInput.Prompt = prompts[0] + ": "
	m.promptInput.Focus()
	return m, textinput.Blink
}

// finishCreate completes the create flow after all prompts are answered.
func (m Model) finishCreate() (tea.Model, tea.Cmd) {
	vs := m.currentView()
	if vs == nil {
		return m, nil
	}

	content, path, err := renderCreateTemplate(vs.cfg.Template, vs.id, m.baseDir, m.createVars)
	if err != nil {
		m.status = fmt.Sprintf("Template error: %v", err)
		return m, nil
	}
	if m.createEncrypted {
		return m.prepareEncryptedCreate(content, path)
	}
	if err := writeNewFile(path, content); err != nil {
		m.status = fmt.Sprintf("Write error: %v", err)
		return m, nil
	}

	// Open in editor
	editor := os.Getenv("EDITOR")
	if editor == "" {
		editor = "vim"
	}
	c := exec.Command(editor, path)
	return m, tea.ExecProcess(c, func(err error) tea.Msg {
		return refreshMsg{}
	})
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

// renderCreateTemplate renders the content template and generates the file path.
// It ensures the resulting content has 'id', 'type', 'tags', and 'created' fields in the frontmatter.
func renderCreateTemplate(contentTmpl, viewID, baseDir string, vars map[string]string) (string, string, error) {
	// Add Slug derived from Title if Title is present
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
	content, err := renderTemplate("content", contentTmpl, vars)
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

func renderTemplate(name, tmplStr string, vars map[string]string) (string, error) {
	tmpl, err := template.New(name).Parse(tmplStr)
	if err != nil {
		return "", err
	}

	var buf strings.Builder
	if err := tmpl.Execute(&buf, vars); err != nil {
		return "", err
	}
	return buf.String(), nil
}

// writeNewFile creates the file and any parent directories.
func writeNewFile(path, content string) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("creating directory %s: %w", dir, err)
	}

	// If file already exists, add a suffix
	if _, err := os.Stat(path); err == nil {
		ext := filepath.Ext(path)
		base := strings.TrimSuffix(path, ext)
		for i := 1; ; i++ {
			candidate := fmt.Sprintf("%s-%d%s", base, i, ext)
			if _, err := os.Stat(candidate); os.IsNotExist(err) {
				path = candidate
				break
			}
		}
	}

	return os.WriteFile(path, []byte(content), 0644)
}

// newAutoFillVars returns the map of auto-fill template variables.
func newAutoFillVars() map[string]string {
	now := time.Now()
	return map[string]string{
		"Date":     now.Format("2006-01-02"),
		"DateTime": now.Format(time.RFC3339),
		"ID":       id.GenerateID(),
		"Year":     now.Format("2006"),
		"Month":    now.Format("01"),
		"Day":      now.Format("02"),
	}
}

// formatYAMLStringList formats a slice of strings as a YAML inline list.
// e.g. ["Author One", "Author Two"] -> `["Author One", "Author Two"]`
func formatYAMLStringList(items []string) string {
	quoted := make([]string, len(items))
	for i, item := range items {
		quoted[i] = fmt.Sprintf("%q", item)
	}
	return "[" + strings.Join(quoted, ", ") + "]"
}
