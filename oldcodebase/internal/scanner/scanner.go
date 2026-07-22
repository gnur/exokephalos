package scanner

import (
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/gnur/exokephalos/internal/markdown"
)

// Item represents a single markdown file with parsed frontmatter.
type Item struct {
	Path        string
	Type        string    // extracted from frontmatter "type"
	Tags        []string  // extracted from frontmatter "tags"
	ID          string    // extracted from frontmatter "id"
	Created     time.Time // normalized from frontmatter "created"
	Frontmatter map[string]interface{}
	Body        string
	ModTime     time.Time
}

// SkipDirs are directory names that should be skipped during scanning and watching.
var SkipDirs = map[string]bool{
	".git":         true,
	".zk":          true,
	".obsidian":    true,
	"node_modules": true,
	".trash":       true,
	"templates":    true,
	".exo":         true,
}

// ScanAll recursively walks dir and parses all .md files.
// Returns all items with their frontmatter and body content.
// Files without valid frontmatter are still included with an empty frontmatter map.
func ScanAll(dir string) ([]Item, error) {
	var items []Item

	err := filepath.WalkDir(dir, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return nil // skip files we can't read
		}

		// Skip hidden directories and known non-content directories
		if d.IsDir() {
			name := d.Name()
			if strings.HasPrefix(name, ".") || SkipDirs[name] {
				return filepath.SkipDir
			}
			return nil
		}

		// Only process .md files
		if filepath.Ext(path) != ".md" {
			return nil
		}

		// Get file info for modification time
		info, err := d.Info()
		if err != nil {
			return nil // skip
		}

		// Parse frontmatter
		fm, body, err := markdown.ParseFrontmatter(path)
		if err != nil {
			// Include the file even if frontmatter parsing fails
			fm = make(map[string]interface{})
		}
		if fm == nil {
			fm = make(map[string]interface{})
		}

		items = append(items, Item{
			Path:        path,
			Frontmatter: fm,
			Body:        body,
			ModTime:     info.ModTime(),
		})

		return nil
	})

	if err != nil {
		return nil, err
	}

	return items, nil
}

// Title extracts a display title from the item.
// It checks the given field name in frontmatter, falls back to filename.
func (item *Item) Title(field string) string {
	if field == "" {
		field = "title"
	}

	if v, ok := item.Frontmatter[field]; ok {
		switch val := v.(type) {
		case string:
			return val
		case time.Time:
			return val.Format("2006-01-02 15:04")
		default:
			return strings.TrimSuffix(filepath.Base(item.Path), ".md")
		}
	}

	return strings.TrimSuffix(filepath.Base(item.Path), ".md")
}

// Subtitle extracts a subtitle from the item using the given field name.
func (item *Item) Subtitle(field string) string {
	if field == "" {
		return ""
	}

	v, ok := item.Frontmatter[field]
	if !ok {
		return ""
	}

	switch val := v.(type) {
	case string:
		return val
	case []interface{}:
		parts := make([]string, 0, len(val))
		for _, p := range val {
			if s, ok := p.(string); ok {
				parts = append(parts, s)
			}
		}
		return strings.Join(parts, ", ")
	case []string:
		return strings.Join(val, ", ")
	case time.Time:
		return val.Format("2006-01-02")
	default:
		return ""
	}
}

// Tags extracts the tags list from the item.
// Prefers the typed Tags field; falls back to frontmatter parsing.
func (item *Item) GetTags() []string {
	if item.Tags != nil {
		return item.Tags
	}

	v, ok := item.Frontmatter["tags"]
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

// Year extracts the year from a frontmatter date field.
// Handles time.Time and string values (tries to parse the first 4 chars as year).
func (item *Item) Year(field string) string {
	v, ok := item.Frontmatter[field]
	if !ok {
		return ""
	}

	switch val := v.(type) {
	case time.Time:
		return val.Format("2006")
	case string:
		if len(val) >= 4 {
			return val[:4]
		}
		return ""
	default:
		return ""
	}
}

// SortValue returns a frontmatter field as a string suitable for ordering.
func (item *Item) SortValue(field string) string {
	return markdown.FMString(item.Frontmatter, field)
}

// SortID returns a stable item identifier for tie-breaking.
func (item *Item) SortID() string {
	if item.ID != "" {
		return item.ID
	}
	if id := markdown.FMString(item.Frontmatter, "id"); id != "" {
		return id
	}
	return strings.TrimSuffix(filepath.Base(item.Path), ".md")
}
