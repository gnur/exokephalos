// Package itemcreate builds the standard on-disk representation for new items.
package itemcreate

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/gnur/exokephalos/internal/id"
	"github.com/gnur/exokephalos/internal/markdown"
)

// Item is a new item ready to be persisted by a repository or sync store.
type Item struct {
	Path        string
	Frontmatter map[string]interface{}
	Body        string
}

// New creates a standard item. Type and title are required because they are
// both part of the item's frontmatter and its destination path.
func New(baseDir, itemType, title, body string) (Item, error) {
	itemType = strings.TrimSpace(itemType)
	title = strings.TrimSpace(title)
	if itemType == "" {
		return Item{}, fmt.Errorf("type is required")
	}
	if strings.ContainsAny(itemType, `/\\`) || itemType == "." || itemType == ".." {
		return Item{}, fmt.Errorf("type must not contain a path separator")
	}
	if title == "" {
		return Item{}, fmt.Errorf("title is required")
	}

	now := time.Now().UTC()
	itemID := id.GenerateID()
	fm := map[string]interface{}{
		"id":      itemID,
		"type":    itemType,
		"title":   title,
		"tags":    []interface{}{},
		"created": now.Format(time.RFC3339),
	}
	if err := Verify(fm, itemType, title); err != nil {
		return Item{}, err
	}

	slug := markdown.Slugify(title)
	if slug == "" {
		slug = itemID
	}
	path := filepath.Join(baseDir, itemType, now.Format("2006"), now.Format("01"), slug+".md")
	path, err := uniquePath(path)
	if err != nil {
		return Item{}, err
	}
	return Item{Path: path, Frontmatter: fm, Body: body}, nil
}

// Verify confirms that persisted frontmatter retains the user-supplied type
// and title required by the standard creation flow.
func Verify(frontmatter map[string]interface{}, itemType, title string) error {
	if markdown.FMString(frontmatter, "type") != itemType {
		return fmt.Errorf("created item has an invalid type")
	}
	if markdown.FMString(frontmatter, "title") != title {
		return fmt.Errorf("created item has an invalid title")
	}
	return nil
}

// Write persists an item, creates its destination directory, and verifies the
// saved frontmatter before reporting success.
func Write(item Item) error {
	if err := os.MkdirAll(filepath.Dir(item.Path), 0755); err != nil {
		return err
	}
	if err := markdown.WriteFrontmatter(item.Path, item.Frontmatter, item.Body); err != nil {
		return err
	}
	frontmatter, _, err := markdown.ParseFrontmatter(item.Path)
	if err != nil {
		return err
	}
	return Verify(frontmatter, markdown.FMString(item.Frontmatter, "type"), markdown.FMString(item.Frontmatter, "title"))
}

func uniquePath(path string) (string, error) {
	if _, err := os.Stat(path); err == nil {
		ext := filepath.Ext(path)
		base := strings.TrimSuffix(path, ext)
		for n := 1; ; n++ {
			candidate := fmt.Sprintf("%s-%d%s", base, n, ext)
			if _, err := os.Stat(candidate); os.IsNotExist(err) {
				return candidate, nil
			} else if err != nil {
				return "", err
			}
		}
	} else if !os.IsNotExist(err) {
		return "", err
	}
	return path, nil
}
