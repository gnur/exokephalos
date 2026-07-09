package markdown

import (
	"bytes"
	"fmt"
	"os"
	"strings"
	"time"
	"unicode"

	"gopkg.in/yaml.v3"
)

// ParseFrontmatter reads a markdown file and returns the YAML frontmatter as a map
// and the body content (after the second ---).
func ParseFrontmatter(path string) (map[string]interface{}, string, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, "", err
	}
	return ParseFrontmatterBytes(data)
}

// ParseFrontmatterBytes parses frontmatter from bytes.
func ParseFrontmatterBytes(data []byte) (map[string]interface{}, string, error) {
	content := string(data)
	if !strings.HasPrefix(content, "---") {
		return nil, content, nil
	}

	end := strings.Index(content[3:], "---")
	if end == -1 {
		return nil, content, nil
	}

	fmRaw := content[3 : end+3]
	body := content[end+6:] // skip past second ---

	var fm map[string]interface{}
	if err := yaml.NewDecoder(bytes.NewReader([]byte(fmRaw))).Decode(&fm); err != nil {
		return nil, body, err
	}

	return fm, strings.TrimPrefix(body, "\n"), nil
}

// WriteFrontmatter creates a markdown file with YAML frontmatter and body.
func WriteFrontmatter(path string, fm map[string]interface{}, body string) error {
	var buf bytes.Buffer
	buf.WriteString("---\n")
	enc := yaml.NewEncoder(&buf)
	enc.SetIndent(2)
	if err := enc.Encode(fm); err != nil {
		return err
	}
	buf.WriteString("---\n")
	buf.WriteString(body)
	return os.WriteFile(path, buf.Bytes(), 0644)
}

// ExtractTags gets the tags field as a string slice from frontmatter.
func ExtractTags(fm map[string]interface{}) []string {
	v, ok := fm["tags"]
	if !ok {
		return nil
	}
	switch tags := v.(type) {
	case []interface{}:
		result := make([]string, 0, len(tags))
		for _, t := range tags {
			if s, ok := t.(string); ok {
				result = append(result, s)
			}
		}
		return result
	case []string:
		return tags
	default:
		return nil
	}
}

// FMString gets a string field from frontmatter, converting other types if necessary.
func FMString(fm map[string]interface{}, key string) string {
	v, ok := fm[key]
	if !ok {
		return ""
	}
	switch val := v.(type) {
	case string:
		return val
	default:
		return fmt.Sprintf("%v", val)
	}
}

// FMInt gets an int field from frontmatter.
func FMInt(fm map[string]interface{}, key string) int {
	v, ok := fm[key]
	if !ok {
		return 0
	}
	switch val := v.(type) {
	case int:
		return val
	case float64:
		return int(val)
	case int64:
		return int(val)
	}
	return 0
}

// Slugify converts a string to a URL-friendly slug.
func Slugify(s string) string {
	var result []rune
	prevDash := false
	for _, r := range strings.ToLower(s) {
		if unicode.IsLetter(r) || unicode.IsDigit(r) {
			result = append(result, r)
			prevDash = false
		} else if !prevDash && len(result) > 0 {
			result = append(result, '-')
			prevDash = true
		}
	}
	// Trim trailing dash
	if len(result) > 0 && result[len(result)-1] == '-' {
		result = result[:len(result)-1]
	}
	// Limit length to 50 characters
	if len(result) > 50 {
		result = result[:50]
	}
	return string(result)
}

// EnsureID checks if the frontmatter in content has an 'id' field.
// If it doesn't, it injects one.
func EnsureID(content, id string) string {
	if !strings.HasPrefix(content, "---") {
		return "---\nid: " + id + "\n---\n" + content
	}
	end := strings.Index(content[3:], "---")
	if end == -1 {
		return content
	}
	fmSection := content[3 : end+3]
	for _, line := range strings.Split(fmSection, "\n") {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "id:") || strings.HasPrefix(trimmed, "id :") {
			return content
		}
	}
	return "---\nid: " + id + "\n" + strings.TrimPrefix(fmSection, "\n") + "---" + content[end+6:]
}

// EnsureRequiredFields parses frontmatter and ensures id, type, tags, and created (or added) are present.
func EnsureRequiredFields(content, defaultID, defaultType string) (string, error) {
	fm, body, err := ParseFrontmatterBytes([]byte(content))
	if err != nil {
		fm = make(map[string]interface{})
	}
	if fm == nil {
		fm = make(map[string]interface{})
	}

	modified := false

	// ID
	if _, ok := fm["id"]; !ok || fm["id"] == "" {
		fm["id"] = defaultID
		modified = true
	}

	// Type
	if _, ok := fm["type"]; !ok || fm["type"] == "" {
		fm["type"] = defaultType
		modified = true
	}

	// Tags
	if _, ok := fm["tags"]; !ok {
		fm["tags"] = []interface{}{}
		modified = true
	}

	// Created
	if _, ok := fm["created"]; !ok {
		if _, okAdded := fm["added"]; !okAdded {
			fm["created"] = time.Now().Format(time.RFC3339)
			modified = true
		}
	}

	if modified || !strings.HasPrefix(content, "---") {
		var buf bytes.Buffer
		buf.WriteString("---\n")
		enc := yaml.NewEncoder(&buf)
		enc.SetIndent(2)
		if err := enc.Encode(fm); err != nil {
			return content, err
		}
		buf.WriteString("---\n\n")
		buf.WriteString(body)
		return buf.String(), nil
	}

	return content, nil
}
