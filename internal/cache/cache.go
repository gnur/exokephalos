package cache

import (
	"fmt"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/fsnotify/fsnotify"
	"github.com/gnur/exokephalos/internal/id"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/scanner"
)


// Cache provides an in-memory cache of all markdown files with fsnotify-based live updates.
// Files without a "type" field in frontmatter are ignored.
// Missing id, created, or tags fields are auto-populated and written back to disk.
type Cache struct {
	mu      sync.RWMutex
	items   map[string]*scanner.Item // key = relative path
	byType  map[string][]string      // type value → []relPath
	byTag   map[string][]string      // tag value → []relPath
	byID    map[string]string        // id (lowercase) → relPath
	baseDir string
	watcher *fsnotify.Watcher
	done    chan struct{}
}

// New creates a new in-memory Cache. It scans all .md files in baseDir,
// auto-populates missing frontmatter fields, builds indexes, and starts
// a background fsnotify watcher for live updates.
func New(baseDir string) (*Cache, error) {
	// Ensure baseDir/.exo/cache exists
	cacheDir := filepath.Join(baseDir, ".exo", "cache")
	if err := os.MkdirAll(cacheDir, 0755); err != nil {
		return nil, fmt.Errorf("creating cache directory: %w", err)
	}

	watcher, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, fmt.Errorf("creating watcher: %w", err)
	}

	c := &Cache{
		items:   make(map[string]*scanner.Item),
		byType:  make(map[string][]string),
		byTag:   make(map[string][]string),
		byID:    make(map[string]string),
		baseDir: baseDir,
		watcher: watcher,
		done:    make(chan struct{}),
	}

	if err := c.scan(); err != nil {
		watcher.Close()
		return nil, fmt.Errorf("initial scan: %w", err)
	}

	go c.watchLoop()

	return c, nil
}

// Close stops the watcher.
func (c *Cache) Close() error {
	close(c.done)
	return c.watcher.Close()
}

// Sync performs a full re-scan from disk and rebuilds all indexes.
func (c *Cache) Sync() error {
	return c.scan()
}

// All returns a snapshot of all cached items (with body).
func (c *Cache) All() ([]scanner.Item, error) {
	c.mu.RLock()
	defer c.mu.RUnlock()

	items := make([]scanner.Item, 0, len(c.items))
	for _, item := range c.items {
		items = append(items, *item)
	}
	return items, nil
}

// GetByPrefix returns all items whose relative path starts with prefix.
func (c *Cache) GetByPrefix(prefix string) ([]scanner.Item, error) {
	c.mu.RLock()
	defer c.mu.RUnlock()

	var items []scanner.Item
	for relPath, item := range c.items {
		if strings.HasPrefix(relPath, prefix) {
			items = append(items, *item)
		}
	}
	return items, nil
}

// Get returns a single item by its relative path.
func (c *Cache) Get(relPath string) (*scanner.Item, error) {
	c.mu.RLock()
	defer c.mu.RUnlock()

	item, ok := c.items[relPath]
	if !ok {
		return nil, fmt.Errorf("not found: %s", relPath)
	}
	cp := *item
	return &cp, nil
}

// GetByID searches the cache for an item by its ID (case-insensitive).
func (c *Cache) GetByID(id string) (*scanner.Item, error) {
	c.mu.RLock()
	defer c.mu.RUnlock()

	relPath, ok := c.byID[id]
	if !ok {
		return nil, fmt.Errorf("id not found: %s", id)
	}
	item, ok := c.items[relPath]
	if !ok {
		return nil, fmt.Errorf("id not found: %s", id)
	}
	cp := *item
	return &cp, nil
}

// GetByType returns all items matching any of the given type values.
func (c *Cache) GetByType(types ...string) ([]scanner.Item, error) {
	c.mu.RLock()
	defer c.mu.RUnlock()

	var items []scanner.Item
	for _, t := range types {
		for _, relPath := range c.byType[t] {
			if item, ok := c.items[relPath]; ok {
				items = append(items, *item)
			}
		}
	}
	return items, nil
}

// GetByTag returns all items that have the given tag.
func (c *Cache) GetByTag(tag string) ([]scanner.Item, error) {
	c.mu.RLock()
	defer c.mu.RUnlock()

	var items []scanner.Item
	for _, relPath := range c.byTag[tag] {
		if item, ok := c.items[relPath]; ok {
			items = append(items, *item)
		}
	}
	return items, nil
}

// NotifyWrite re-reads a file from disk and updates the cache.
func (c *Cache) NotifyWrite(absPath string) error {
	if filepath.Ext(absPath) != ".md" {
		return nil
	}
	relPath, err := filepath.Rel(c.baseDir, absPath)
	if err != nil {
		return err
	}
	return c.indexFile(absPath, relPath)
}

// NotifyDelete removes a file from the cache.
func (c *Cache) NotifyDelete(absPath string) error {
	if filepath.Ext(absPath) != ".md" {
		return nil
	}
	relPath, err := filepath.Rel(c.baseDir, absPath)
	if err != nil {
		return err
	}

	c.mu.Lock()
	defer c.mu.Unlock()

	c.removeFromIndexes(relPath)
	delete(c.items, relPath)
	return nil
}

// --- Internal ---

// scan walks the filesystem, indexes all .md files, and rebuilds the cache.
func (c *Cache) scan() error {
	c.mu.Lock()
	defer c.mu.Unlock()

	newItems := make(map[string]*scanner.Item)
	newByType := make(map[string][]string)
	newByTag := make(map[string][]string)
	newByID := make(map[string]string)

	type legacyItem struct {
		absPath string
		relPath string
		item    *scanner.Item
	}
	var legacyItems []legacyItem

	err := filepath.WalkDir(c.baseDir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}

		if d.IsDir() {
			name := d.Name()
			if strings.HasPrefix(name, ".") || scanner.SkipDirs[name] {
				return filepath.SkipDir
			}
			if watchErr := c.watcher.Add(path); watchErr != nil {
				log.Printf("cache: failed to watch %s: %v", path, watchErr)
			}
			return nil
		}

		if filepath.Ext(path) != ".md" {
			return nil
		}

		relPath, err := filepath.Rel(c.baseDir, path)
		if err != nil {
			return nil
		}

		item, err := c.readAndPopulate(path, relPath)
		if err != nil || item == nil {
			return nil // skip files without type or with errors
		}

		// Check if ID is in legacy 9-character format
		if item.ID != "" && len(item.ID) == 9 {
			legacyItems = append(legacyItems, legacyItem{
				absPath: path,
				relPath: relPath,
				item:    item,
			})
		} else {
			newItems[relPath] = item
		}

		return nil
	})

	if err != nil {
		return err
	}

	// Migrate and move legacy items
	for _, li := range legacyItems {
		item := li.item
		oldAbsPath := li.absPath

		// 1. Generate new 7-char lowercase base32 ID based on the creation timestamp
		newID := id.GenerateIDFromTime(item.Created)
		item.ID = newID
		item.Frontmatter["id"] = newID

		// 2. Resolve title for slugified filename
		titleVal := markdown.FMString(item.Frontmatter, "title")
		if titleVal == "" {
			oldFilename := filepath.Base(oldAbsPath)
			oldBase := strings.TrimSuffix(oldFilename, ".md")
			if len(oldBase) > 10 && oldBase[9] == '-' {
				titleVal = oldBase[10:]
			}
		}

		slug := ""
		if titleVal != "" {
			slug = markdown.Slugify(titleVal)
		}

		var newFilename string
		if slug != "" {
			newFilename = newID + "-" + slug + ".md"
		} else {
			newFilename = newID + ".md"
		}

		newDestDir := filepath.Join(c.baseDir, newID[:3])
		newAbsPath := filepath.Clean(filepath.Join(newDestDir, newFilename))

		// 3. Create destination directory if needed
		if err := os.MkdirAll(newDestDir, 0755); err != nil {
			log.Printf("cache: failed to create directory %s: %v", newDestDir, err)
			// fallback: rewrite on old path with new ID
			if err := markdown.WriteFrontmatter(oldAbsPath, item.Frontmatter, item.Body); err != nil {
				log.Printf("cache: failed to rewrite %s with new ID: %v", oldAbsPath, err)
			}
			newItems[li.relPath] = item
			continue
		}

		// 4. Write to new path
		if err := markdown.WriteFrontmatter(newAbsPath, item.Frontmatter, item.Body); err != nil {
			log.Printf("cache: failed to write migrated file to %s: %v", newAbsPath, err)
			// fallback: rewrite on old path with new ID
			if err := markdown.WriteFrontmatter(oldAbsPath, item.Frontmatter, item.Body); err != nil {
				log.Printf("cache: failed to rewrite %s with new ID: %v", oldAbsPath, err)
			}
			newItems[li.relPath] = item
			continue
		}

		// 5. Delete old file
		if err := os.Remove(oldAbsPath); err != nil {
			log.Printf("cache: failed to remove old file %s: %v", oldAbsPath, err)
		}

		// 6. Update item path metadata
		item.Path = newAbsPath
		newRelPath, err := filepath.Rel(c.baseDir, newAbsPath)
		if err != nil {
			newRelPath = filepath.Join(newID[:3], newFilename)
		}

		newItems[newRelPath] = item
		log.Printf("cache: migrated legacy item %s -> %s (new ID: %s)", li.relPath, newRelPath, newID)
	}

	// Rebuild indexes
	for relPath, item := range newItems {
		newByType[item.Type] = append(newByType[item.Type], relPath)
		for _, tag := range item.Tags {
			newByTag[tag] = append(newByTag[tag], relPath)
		}
		if item.ID != "" {
			newByID[item.ID] = relPath
		}
	}

	c.items = newItems
	c.byType = newByType
	c.byTag = newByTag
	c.byID = newByID

	return nil
}

// indexFile reads a single file and updates the cache + indexes.
func (c *Cache) indexFile(absPath, relPath string) error {
	c.mu.Lock()
	defer c.mu.Unlock()

	item, err := c.readAndPopulate(absPath, relPath)
	if err != nil {
		return err
	}

	if item == nil {
		// No type field: remove from cache if it was there before.
		c.removeFromIndexes(relPath)
		delete(c.items, relPath)
		return nil
	}

	// Remove old index entries.
	c.removeFromIndexes(relPath)

	// Add new item and index entries.
	c.items[relPath] = item
	c.byType[item.Type] = append(c.byType[item.Type], relPath)
	for _, tag := range item.Tags {
		c.byTag[tag] = append(c.byTag[tag], relPath)
	}
	if item.ID != "" {
		c.byID[item.ID] = relPath
	}

	return nil
}

// readAndPopulate reads a file, parses frontmatter, auto-populates missing fields,
// and returns a fully populated Item. Returns nil if the file has no "type" field.
func (c *Cache) readAndPopulate(absPath, relPath string) (*scanner.Item, error) {
	info, err := os.Stat(absPath)
	if err != nil {
		return nil, err
	}

	content, err := os.ReadFile(absPath)
	if err != nil {
		return nil, err
	}

	fm, body, err := markdown.ParseFrontmatterBytes(content)
	if err != nil {
		log.Printf("cache: failed to parse frontmatter in %s: %v", relPath, err)
		fm = make(map[string]interface{})
	}
	if fm == nil {
		fm = make(map[string]interface{})
	}

	// Type is required — skip files without it.
	typeVal := markdown.FMString(fm, "type")
	if typeVal == "" {
		return nil, nil
	}

	// Auto-populate missing fields and track if we need to rewrite.
	needsRewrite := false

	// ID
	idVal := markdown.FMString(fm, "id")
	if idVal == "" {
		idVal = id.GenerateID()
		fm["id"] = idVal
		needsRewrite = true
	}

	// Created
	createdVal := fmTime(fm, "created")
	if createdVal.IsZero() {
		createdVal = info.ModTime()
		fm["created"] = createdVal.Format(time.RFC3339)
		needsRewrite = true
	}

	// Tags
	tags := markdown.ExtractTags(fm)
	if _, hasTags := fm["tags"]; !hasTags {
		fm["tags"] = []interface{}{}
		tags = []string{}
		needsRewrite = true
	}

	// Rewrite the file if we auto-populated any fields.
	if needsRewrite {
		if err := markdown.WriteFrontmatter(absPath, fm, body); err != nil {
			log.Printf("cache: failed to rewrite %s: %v", relPath, err)
		}
	}

	return &scanner.Item{
		Path:        absPath,
		Type:        typeVal,
		Tags:        tags,
		ID:          idVal,
		Created:     createdVal,
		Frontmatter: fm,
		Body:        body,
		ModTime:     info.ModTime(),
	}, nil
}

// removeFromIndexes removes a relPath from the type and tag indexes.
func (c *Cache) removeFromIndexes(relPath string) {
	old, ok := c.items[relPath]
	if !ok {
		return
	}

	// Remove from type index.
	c.byType[old.Type] = removeFromSlice(c.byType[old.Type], relPath)
	if len(c.byType[old.Type]) == 0 {
		delete(c.byType, old.Type)
	}

	// Remove from tag indexes.
	for _, tag := range old.Tags {
		c.byTag[tag] = removeFromSlice(c.byTag[tag], relPath)
		if len(c.byTag[tag]) == 0 {
			delete(c.byTag, tag)
		}
	}

	// Remove from ID index.
	if old.ID != "" {
		delete(c.byID, old.ID)
	}
}

// --- Watcher ---

func (c *Cache) watchLoop() {
	for {
		select {
		case <-c.done:
			return
		case event, ok := <-c.watcher.Events:
			if !ok {
				return
			}
			c.handleEvent(event)
		case err, ok := <-c.watcher.Errors:
			if !ok {
				return
			}
			log.Printf("cache: watcher error: %v", err)
		}
	}
}

func (c *Cache) handleEvent(event fsnotify.Event) {
	path := event.Name

	relPath, err := filepath.Rel(c.baseDir, path)
	if err != nil {
		return
	}

	// Skip hidden directories and known skip dirs.
	parts := strings.Split(relPath, string(filepath.Separator))
	for _, part := range parts {
		if strings.HasPrefix(part, ".") || scanner.SkipDirs[part] {
			return
		}
	}

	switch {
	case event.Has(fsnotify.Create):
		info, err := os.Stat(path)
		if err != nil {
			return
		}
		if info.IsDir() {
			if watchErr := c.watcher.Add(path); watchErr != nil {
				log.Printf("cache: failed to watch new directory %s: %v", path, watchErr)
			}
			c.scanNewDir(path)
			return
		}
		if filepath.Ext(path) == ".md" {
			c.indexFile(path, relPath)
		}

	case event.Has(fsnotify.Write):
		if filepath.Ext(path) == ".md" {
			c.indexFile(path, relPath)
		}

	case event.Has(fsnotify.Remove) || event.Has(fsnotify.Rename):
		if filepath.Ext(path) == ".md" {
			c.mu.Lock()
			c.removeFromIndexes(relPath)
			delete(c.items, relPath)
			c.mu.Unlock()
		}
	}
}

func (c *Cache) scanNewDir(dir string) {
	err := filepath.WalkDir(dir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			name := d.Name()
			if path != dir && (strings.HasPrefix(name, ".") || scanner.SkipDirs[name]) {
				return filepath.SkipDir
			}
			if watchErr := c.watcher.Add(path); watchErr != nil {
				log.Printf("cache: failed to watch subdirectory %s: %v", path, watchErr)
			}
			return nil
		}
		if filepath.Ext(path) != ".md" {
			return nil
		}
		relPath, err := filepath.Rel(c.baseDir, path)
		if err != nil {
			return nil
		}
		c.indexFile(path, relPath)
		return nil
	})
	if err != nil {
		log.Printf("cache: failed to walk new directory %s: %v", dir, err)
	}
}

// --- Helpers ---


func fmTime(fm map[string]interface{}, key string) time.Time {
	v, ok := fm[key]
	if !ok {
		return time.Time{}
	}
	switch val := v.(type) {
	case time.Time:
		return val
	case string:
		// Try common date formats
		for _, layout := range []string{
			"2006-01-02T15:04:05",
			"2006-01-02",
			time.RFC3339,
		} {
			if t, err := time.Parse(layout, val); err == nil {
				return t
			}
		}
	}
	return time.Time{}
}


func removeFromSlice(s []string, val string) []string {
	for i, v := range s {
		if v == val {
			return append(s[:i], s[i+1:]...)
		}
	}
	return s
}
