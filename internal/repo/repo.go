package repo

import (
	"os"
	"path/filepath"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/scanner"
)

type Repo struct {
	BaseDir string
	Cache   *cache.Cache
}

func New(baseDir string, c *cache.Cache) *Repo {
	return &Repo{BaseDir: baseDir, Cache: c}
}

// ReadRaw reads the raw content of a file.
func (r *Repo) ReadRaw(path string) (string, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	return string(data), nil
}

// WriteRaw writes raw content to a file.
func (r *Repo) WriteRaw(path, content string) error {
	return os.WriteFile(path, []byte(content), 0644)
}

// --- Generic Data Access ---

// GetItem retrieves a single item by its path.
func (r *Repo) GetItem(path string) (*scanner.Item, error) {
	fm, body, err := markdown.ParseFrontmatter(path)
	if err != nil {
		return nil, err
	}
	return &scanner.Item{
		Path:        path,
		Frontmatter: fm,
		Body:        body,
	}, nil
}

// ListItems retrieves all items matching a cache prefix.
func (r *Repo) ListItems(prefix string) ([]scanner.Item, error) {
	return r.Cache.GetByPrefix(prefix)
}

// CreateItem creates a new item at the given path with frontmatter and body.
func (r *Repo) CreateItem(path string, fm map[string]interface{}, body string) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}
	if err := markdown.WriteFrontmatter(path, fm, body); err != nil {
		return err
	}
	r.Cache.NotifyWrite(path)
	return nil
}

// UpdateItem updates an existing item's frontmatter and body.
func (r *Repo) UpdateItem(path string, fm map[string]interface{}, body string) error {
	if err := markdown.WriteFrontmatter(path, fm, body); err != nil {
		return err
	}
	r.Cache.NotifyWrite(path)
	return nil
}

// DeleteItem removes an item from disk and notifies the cache.
func (r *Repo) DeleteItem(path string) error {
	if err := os.Remove(path); err != nil {
		return err
	}
	r.Cache.NotifyDelete(path)
	return nil
}
