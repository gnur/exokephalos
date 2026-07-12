package cache

import (
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/gnur/exokephalos/internal/id"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/scanner"
	_ "modernc.org/sqlite"
)

// OutboxEntry is a local sync history row. Entries are retained after a
// successful push so the TUI can show both pending work and recent history.
type OutboxEntry struct {
	ID            int64
	Op            string
	TargetKind    string
	TargetID      string
	Path          string
	Status        string
	Attempts      int
	LastError     string
	CreatedAt     time.Time
	LastAttemptAt time.Time
	Payload       string
}

// Cache is a SQLite-backed cache of local markdown files and sync state.
type Cache struct {
	mu      sync.Mutex
	db      *sql.DB
	baseDir string
}

// New creates a SQLite-backed cache in baseDir/.exo/cache.sqlite and scans the
// local filesystem into it. Files without a "type" field are ignored.
func New(baseDir string) (*Cache, error) {
	cacheDir := filepath.Join(baseDir, ".exo")
	if err := os.MkdirAll(cacheDir, 0755); err != nil {
		return nil, fmt.Errorf("creating .exo directory: %w", err)
	}

	db, err := sql.Open("sqlite", filepath.Join(cacheDir, "cache.sqlite"))
	if err != nil {
		return nil, fmt.Errorf("opening cache sqlite: %w", err)
	}
	c := &Cache{db: db, baseDir: baseDir}
	if err := c.migrate(); err != nil {
		_ = db.Close()
		return nil, err
	}
	if err := c.Sync(); err != nil {
		_ = db.Close()
		return nil, err
	}
	return c, nil
}

func (c *Cache) Close() error {
	if c == nil || c.db == nil {
		return nil
	}
	return c.db.Close()
}

func (c *Cache) DB() *sql.DB {
	return c.db
}

func (c *Cache) migrate() error {
	stmts := []string{
		`PRAGMA journal_mode = WAL`,
		`CREATE TABLE IF NOT EXISTS items (
			id TEXT PRIMARY KEY,
			path TEXT NOT NULL UNIQUE,
			frontmatter TEXT NOT NULL,
			body TEXT NOT NULL,
			type TEXT NOT NULL,
			tags TEXT NOT NULL,
			created TEXT NOT NULL,
			mod_time INTEGER NOT NULL,
			content_hash TEXT NOT NULL,
			deleted_at TEXT NOT NULL DEFAULT '',
			server_revision INTEGER NOT NULL DEFAULT 0,
			sync_state TEXT NOT NULL DEFAULT ''
		)`,
		`CREATE INDEX IF NOT EXISTS idx_items_path ON items(path)`,
		`CREATE INDEX IF NOT EXISTS idx_items_type ON items(type)`,
		`CREATE TABLE IF NOT EXISTS outbox (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			op TEXT NOT NULL,
			target_kind TEXT NOT NULL,
			target_id TEXT NOT NULL,
			path TEXT NOT NULL,
			payload TEXT NOT NULL,
			status TEXT NOT NULL,
			attempts INTEGER NOT NULL DEFAULT 0,
			last_error TEXT NOT NULL DEFAULT '',
			created_at TEXT NOT NULL,
			last_attempt_at TEXT NOT NULL DEFAULT ''
		)`,
		`CREATE INDEX IF NOT EXISTS idx_outbox_status ON outbox(status, id)`,
		`CREATE TABLE IF NOT EXISTS meta (
			key TEXT PRIMARY KEY,
			value TEXT NOT NULL
		)`,
	}
	for _, stmt := range stmts {
		if _, err := c.db.Exec(stmt); err != nil {
			return fmt.Errorf("cache migration: %w", err)
		}
	}
	return nil
}

// Sync scans markdown files, updates SQLite, and tombstones rows whose files
// disappeared. It also performs the existing legacy ID migration.
func (c *Cache) Sync() error {
	c.mu.Lock()
	defer c.mu.Unlock()

	seen := make(map[string]bool)
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
			return nil
		}
		if filepath.Ext(path) != ".md" {
			return nil
		}
		relPath, err := filepath.Rel(c.baseDir, path)
		if err != nil {
			return nil
		}
		item, contentHash, err := c.readAndPopulate(path, relPath)
		if err != nil || item == nil {
			return nil
		}
		if item.ID != "" && len(item.ID) == 9 {
			legacyItems = append(legacyItems, legacyItem{absPath: path, relPath: relPath, item: item})
			return nil
		}
		seen[relPath] = true
		if c.isSyncStartedLocked() && c.itemChangedLocked(item.ID, contentHash) {
			_ = c.enqueueItemLocked("upsert_item", item, relPath)
		}
		_ = c.upsertItem(item, contentHash, "")
		return nil
	})
	if err != nil {
		return err
	}

	for _, li := range legacyItems {
		item, newPath, err := c.migrateLegacyItem(li.absPath, li.item)
		if err != nil {
			log.Printf("cache: legacy migration failed for %s: %v", li.relPath, err)
			continue
		}
		relPath, err := filepath.Rel(c.baseDir, newPath)
		if err != nil {
			continue
		}
		seen[relPath] = true
		contentHash := hashItem(item.Frontmatter, item.Body)
		_ = c.upsertItem(item, contentHash, "")
	}

	rows, err := c.db.Query(`SELECT path FROM items WHERE deleted_at = ''`)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var relPath string
		if err := rows.Scan(&relPath); err != nil {
			return err
		}
		if !seen[relPath] {
			if c.isSyncStartedLocked() {
				var idVal string
				_ = c.db.QueryRow(`SELECT id FROM items WHERE path = ?`, relPath).Scan(&idVal)
				payload := fmt.Sprintf(`{"op":"delete_item","target_kind":"item","id":%q,"path":%q}`, idVal, relPath)
				_ = c.EnqueueOutbox("delete_item", "item", idVal, relPath, payload)
			}
			if _, err := c.db.Exec(`UPDATE items SET deleted_at = ? WHERE path = ?`, time.Now().UTC().Format(time.RFC3339Nano), relPath); err != nil {
				return err
			}
		}
	}
	return rows.Err()
}

func (c *Cache) All() ([]scanner.Item, error) {
	_ = c.Sync()
	return c.queryItems(`SELECT id, path, frontmatter, body, type, tags, created, mod_time FROM items WHERE deleted_at = ''`, nil)
}

func (c *Cache) GetByPrefix(prefix string) ([]scanner.Item, error) {
	_ = c.Sync()
	return c.queryItems(`SELECT id, path, frontmatter, body, type, tags, created, mod_time FROM items WHERE deleted_at = '' AND path LIKE ?`, []interface{}{prefix + "%"})
}

func (c *Cache) Get(relPath string) (*scanner.Item, error) {
	_ = c.Sync()
	items, err := c.queryItems(`SELECT id, path, frontmatter, body, type, tags, created, mod_time FROM items WHERE deleted_at = '' AND path = ?`, []interface{}{relPath})
	if err != nil {
		return nil, err
	}
	if len(items) == 0 {
		return nil, fmt.Errorf("not found: %s", relPath)
	}
	return &items[0], nil
}

func (c *Cache) GetByID(id string) (*scanner.Item, error) {
	_ = c.Sync()
	items, err := c.queryItems(`SELECT id, path, frontmatter, body, type, tags, created, mod_time FROM items WHERE deleted_at = '' AND lower(id) = lower(?)`, []interface{}{id})
	if err != nil {
		return nil, err
	}
	if len(items) == 0 {
		return nil, fmt.Errorf("id not found: %s", id)
	}
	return &items[0], nil
}

func (c *Cache) GetByType(types ...string) ([]scanner.Item, error) {
	_ = c.Sync()
	if len(types) == 0 {
		return nil, nil
	}
	placeholders := strings.TrimRight(strings.Repeat("?,", len(types)), ",")
	args := make([]interface{}, 0, len(types))
	for _, typ := range types {
		args = append(args, typ)
	}
	return c.queryItems(`SELECT id, path, frontmatter, body, type, tags, created, mod_time FROM items WHERE deleted_at = '' AND type IN (`+placeholders+`)`, args)
}

func (c *Cache) GetByTag(tag string) ([]scanner.Item, error) {
	_ = c.Sync()
	items, err := c.All()
	if err != nil {
		return nil, err
	}
	var result []scanner.Item
	for _, item := range items {
		for _, itemTag := range item.Tags {
			if itemTag == tag {
				result = append(result, item)
				break
			}
		}
	}
	return result, nil
}

func (c *Cache) NotifyWrite(absPath string) error {
	return c.notifyWrite(absPath, true)
}

func (c *Cache) NotifyWriteNoOutbox(absPath string) error {
	return c.notifyWrite(absPath, false)
}

func (c *Cache) notifyWrite(absPath string, enqueue bool) error {
	if filepath.Ext(absPath) != ".md" {
		return nil
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	relPath, err := filepath.Rel(c.baseDir, absPath)
	if err != nil {
		return err
	}
	item, contentHash, err := c.readAndPopulate(absPath, relPath)
	if err != nil {
		return err
	}
	if item == nil {
		_, err = c.db.Exec(`UPDATE items SET deleted_at = ? WHERE path = ?`, time.Now().UTC().Format(time.RFC3339Nano), relPath)
		return err
	}
	if enqueue && c.isSyncStartedLocked() && item.ID != "" && c.itemChangedLocked(item.ID, contentHash) {
		_ = c.enqueueItemLocked("upsert_item", item, relPath)
	}
	return c.upsertItem(item, contentHash, "")
}

func (c *Cache) NotifyDelete(absPath string) error {
	if filepath.Ext(absPath) != ".md" {
		return nil
	}
	relPath, err := filepath.Rel(c.baseDir, absPath)
	if err != nil {
		return err
	}
	if c.IsSyncStarted() {
		var idVal string
		_ = c.db.QueryRow(`SELECT id FROM items WHERE path = ?`, relPath).Scan(&idVal)
		payload := fmt.Sprintf(`{"op":"delete_item","target_kind":"item","id":%q,"path":%q}`, idVal, relPath)
		_ = c.EnqueueOutbox("delete_item", "item", idVal, relPath, payload)
	}
	_, err = c.db.Exec(`UPDATE items SET deleted_at = ? WHERE path = ?`, time.Now().UTC().Format(time.RFC3339Nano), relPath)
	return err
}

func (c *Cache) NotifyDeleteNoOutbox(absPath string) error {
	if filepath.Ext(absPath) != ".md" {
		return nil
	}
	relPath, err := filepath.Rel(c.baseDir, absPath)
	if err != nil {
		return err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	_, err = c.db.Exec(`UPDATE items SET deleted_at = ? WHERE path = ?`, time.Now().UTC().Format(time.RFC3339Nano), relPath)
	return err
}

func (c *Cache) IsSyncStarted() bool {
	v, _ := c.Meta("sync_started")
	return v == "true"
}

func (c *Cache) isSyncStartedLocked() bool {
	var value string
	_ = c.db.QueryRow(`SELECT value FROM meta WHERE key = 'sync_started'`).Scan(&value)
	return value == "true"
}

func (c *Cache) SetSyncStarted(started bool) error {
	val := "false"
	if started {
		val = "true"
	}
	return c.SetMeta("sync_started", val)
}

func (c *Cache) Meta(key string) (string, error) {
	var value string
	err := c.db.QueryRow(`SELECT value FROM meta WHERE key = ?`, key).Scan(&value)
	if err == sql.ErrNoRows {
		return "", nil
	}
	return value, err
}

func (c *Cache) SetMeta(key, value string) error {
	_, err := c.db.Exec(`INSERT INTO meta(key, value) VALUES(?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value`, key, value)
	return err
}

func (c *Cache) EnqueueOutbox(op, targetKind, targetID, path, payload string) error {
	now := time.Now().UTC().Format(time.RFC3339Nano)
	_, err := c.db.Exec(`INSERT INTO outbox(op, target_kind, target_id, path, payload, status, created_at) VALUES(?, ?, ?, ?, ?, 'pending', ?)`, op, targetKind, targetID, path, payload, now)
	return err
}

func (c *Cache) itemChangedLocked(idVal, contentHash string) bool {
	var oldHash string
	err := c.db.QueryRow(`SELECT content_hash FROM items WHERE id = ? AND deleted_at = ''`, idVal).Scan(&oldHash)
	return err == sql.ErrNoRows || oldHash != contentHash
}

func (c *Cache) enqueueItemLocked(op string, item *scanner.Item, relPath string) error {
	payloadMap := map[string]interface{}{
		"op":          op,
		"target_kind": "item",
		"id":          item.ID,
		"path":        relPath,
		"frontmatter": item.Frontmatter,
		"body":        item.Body,
	}
	payload, _ := json.Marshal(payloadMap)
	return c.EnqueueOutbox(op, "item", item.ID, relPath, string(payload))
}

func (c *Cache) OutboxEntries(limit int) ([]OutboxEntry, error) {
	return c.OutboxEntriesByStatus("", limit)
}

func (c *Cache) OutboxEntriesByStatus(status string, limit int) ([]OutboxEntry, error) {
	if limit <= 0 {
		limit = 200
	}
	query := `SELECT id, op, target_kind, target_id, path, payload, status, attempts, last_error, created_at, last_attempt_at FROM outbox`
	var args []interface{}
	if status != "" && status != "all" {
		query += ` WHERE status = ?`
		args = append(args, status)
	}
	query += ` ORDER BY id DESC LIMIT ?`
	args = append(args, limit)
	rows, err := c.db.Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var entries []OutboxEntry
	for rows.Next() {
		var e OutboxEntry
		var created, attempted string
		if err := rows.Scan(&e.ID, &e.Op, &e.TargetKind, &e.TargetID, &e.Path, &e.Payload, &e.Status, &e.Attempts, &e.LastError, &created, &attempted); err != nil {
			return nil, err
		}
		e.CreatedAt = parseTime(created)
		e.LastAttemptAt = parseTime(attempted)
		entries = append(entries, e)
	}
	return entries, rows.Err()
}

func (c *Cache) RetryOutbox(id int64) error {
	_, err := c.db.Exec(`UPDATE outbox SET status = 'pending', last_error = '' WHERE id = ?`, id)
	return err
}

func (c *Cache) RetryFailedOutbox() (int64, error) {
	res, err := c.db.Exec(`UPDATE outbox SET status = 'pending', last_error = '' WHERE status = 'failed'`)
	if err != nil {
		return 0, err
	}
	return res.RowsAffected()
}

func (c *Cache) PendingOutbox(limit int) ([]OutboxEntry, error) {
	if limit <= 0 {
		limit = 100
	}
	now := time.Now().UTC()
	t1 := now.Add(-10 * time.Second).Format(time.RFC3339Nano)
	t2 := now.Add(-1 * time.Minute).Format(time.RFC3339Nano)
	t3 := now.Add(-5 * time.Minute).Format(time.RFC3339Nano)
	t4 := now.Add(-15 * time.Minute).Format(time.RFC3339Nano)

	rows, err := c.db.Query(`
		SELECT id, op, target_kind, target_id, path, payload, status, attempts, last_error, created_at, last_attempt_at 
		FROM outbox 
		WHERE status = 'pending' OR (status = 'failed' AND attempts < 5 AND (
			(attempts = 1 AND last_attempt_at < ?) OR
			(attempts = 2 AND last_attempt_at < ?) OR
			(attempts = 3 AND last_attempt_at < ?) OR
			(attempts = 4 AND last_attempt_at < ?)
		)) 
		ORDER BY id ASC LIMIT ?`, t1, t2, t3, t4, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var entries []OutboxEntry
	for rows.Next() {
		var e OutboxEntry
		var created, attempted string
		if err := rows.Scan(&e.ID, &e.Op, &e.TargetKind, &e.TargetID, &e.Path, &e.Payload, &e.Status, &e.Attempts, &e.LastError, &created, &attempted); err != nil {
			return nil, err
		}
		e.CreatedAt = parseTime(created)
		e.LastAttemptAt = parseTime(attempted)
		entries = append(entries, e)
	}
	return entries, rows.Err()
}

func (c *Cache) MarkOutboxSynced(id int64) error {
	_, err := c.db.Exec(`UPDATE outbox SET status = 'synced', last_error = '', last_attempt_at = ? WHERE id = ?`, time.Now().UTC().Format(time.RFC3339Nano), id)
	return err
}

func (c *Cache) MarkOutboxFailed(id int64, msg string) error {
	_, err := c.db.Exec(`UPDATE outbox SET status = 'failed', attempts = attempts + 1, last_error = ?, last_attempt_at = ? WHERE id = ?`, msg, time.Now().UTC().Format(time.RFC3339Nano), id)
	return err
}

func (c *Cache) queryItems(query string, args []interface{}) ([]scanner.Item, error) {
	rows, err := c.db.Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var items []scanner.Item
	for rows.Next() {
		item, err := c.scanItemRow(rows)
		if err != nil {
			return nil, err
		}
		items = append(items, item)
	}
	return items, rows.Err()
}

func (c *Cache) scanItemRow(rows *sql.Rows) (scanner.Item, error) {
	var idVal, relPath, fmJSON, body, typ, tagsJSON, createdStr string
	var modUnix int64
	if err := rows.Scan(&idVal, &relPath, &fmJSON, &body, &typ, &tagsJSON, &createdStr, &modUnix); err != nil {
		return scanner.Item{}, err
	}
	var fm map[string]interface{}
	if err := json.Unmarshal([]byte(fmJSON), &fm); err != nil {
		return scanner.Item{}, err
	}
	var tags []string
	if err := json.Unmarshal([]byte(tagsJSON), &tags); err != nil {
		return scanner.Item{}, err
	}
	return scanner.Item{
		Path:        filepath.Join(c.baseDir, relPath),
		Type:        typ,
		Tags:        tags,
		ID:          idVal,
		Created:     parseTime(createdStr),
		Frontmatter: fm,
		Body:        body,
		ModTime:     time.Unix(0, modUnix),
	}, nil
}

func (c *Cache) upsertItem(item *scanner.Item, contentHash, syncState string) error {
	relPath, err := filepath.Rel(c.baseDir, item.Path)
	if err != nil {
		relPath = item.Path
	}
	fmJSON, err := json.Marshal(item.Frontmatter)
	if err != nil {
		return err
	}
	tagsJSON, err := json.Marshal(item.Tags)
	if err != nil {
		return err
	}
	if syncState == "" {
		var existing string
		_ = c.db.QueryRow(`SELECT sync_state FROM items WHERE id = ?`, item.ID).Scan(&existing)
		syncState = existing
	}
	_, err = c.db.Exec(`
		INSERT INTO items(id, path, frontmatter, body, type, tags, created, mod_time, content_hash, deleted_at, sync_state)
		VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, '', ?)
		ON CONFLICT(id) DO UPDATE SET
			path = excluded.path,
			frontmatter = excluded.frontmatter,
			body = excluded.body,
			type = excluded.type,
			tags = excluded.tags,
			created = excluded.created,
			mod_time = excluded.mod_time,
			content_hash = excluded.content_hash,
			deleted_at = '',
			sync_state = excluded.sync_state
	`, item.ID, relPath, string(fmJSON), item.Body, item.Type, string(tagsJSON), item.Created.Format(time.RFC3339Nano), item.ModTime.UnixNano(), contentHash, syncState)
	return err
}

func (c *Cache) readAndPopulate(absPath, relPath string) (*scanner.Item, string, error) {
	info, err := os.Stat(absPath)
	if err != nil {
		return nil, "", err
	}
	content, err := os.ReadFile(absPath)
	if err != nil {
		return nil, "", err
	}
	fm, body, err := markdown.ParseFrontmatterBytes(content)
	if err != nil {
		log.Printf("cache: failed to parse frontmatter in %s: %v", relPath, err)
		fm = make(map[string]interface{})
	}
	if fm == nil {
		fm = make(map[string]interface{})
	}
	typeVal := markdown.FMString(fm, "type")
	if typeVal == "" {
		return nil, "", nil
	}

	needsRewrite := false
	idVal := markdown.FMString(fm, "id")
	if idVal == "" {
		idVal = id.GenerateID()
		fm["id"] = idVal
		needsRewrite = true
	}
	createdVal := fmTime(fm, "created")
	if createdVal.IsZero() {
		createdVal = info.ModTime()
		fm["created"] = createdVal.Format(time.RFC3339)
		needsRewrite = true
	}
	tags := markdown.ExtractTags(fm)
	if _, hasTags := fm["tags"]; !hasTags {
		fm["tags"] = []interface{}{}
		tags = []string{}
		needsRewrite = true
	}
	if needsRewrite {
		if err := markdown.WriteFrontmatter(absPath, fm, body); err != nil {
			log.Printf("cache: failed to rewrite %s: %v", relPath, err)
		}
		content, _ = os.ReadFile(absPath)
		info, _ = os.Stat(absPath)
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
	}, sha(content), nil
}

func (c *Cache) migrateLegacyItem(oldAbsPath string, item *scanner.Item) (*scanner.Item, string, error) {
	newID := id.GenerateIDFromTime(item.Created)
	item.ID = newID
	item.Frontmatter["id"] = newID

	titleVal := markdown.FMString(item.Frontmatter, "title")
	if titleVal == "" {
		oldFilename := filepath.Base(oldAbsPath)
		oldBase := strings.TrimSuffix(oldFilename, ".md")
		if len(oldBase) > 10 && oldBase[9] == '-' {
			titleVal = oldBase[10:]
		}
	}
	slug := markdown.Slugify(titleVal)
	newFilename := newID + ".md"
	if slug != "" {
		newFilename = newID + "-" + slug + ".md"
	}
	newDestDir := filepath.Join(c.baseDir, newID[:3])
	newAbsPath := filepath.Join(newDestDir, newFilename)
	if err := os.MkdirAll(newDestDir, 0755); err != nil {
		return nil, "", err
	}
	if err := markdown.WriteFrontmatter(newAbsPath, item.Frontmatter, item.Body); err != nil {
		return nil, "", err
	}
	if err := os.Remove(oldAbsPath); err != nil {
		log.Printf("cache: failed to remove old file %s: %v", oldAbsPath, err)
	}
	info, err := os.Stat(newAbsPath)
	if err == nil {
		item.ModTime = info.ModTime()
	}
	item.Path = newAbsPath
	return item, newAbsPath, nil
}

func hashItem(fm map[string]interface{}, body string) string {
	b, _ := json.Marshal(fm)
	sum := sha256.Sum256(append(b, []byte(body)...))
	return hex.EncodeToString(sum[:])
}

func sha(data []byte) string {
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])
}

func fmTime(fm map[string]interface{}, key string) time.Time {
	v, ok := fm[key]
	if !ok {
		return time.Time{}
	}
	switch val := v.(type) {
	case time.Time:
		return val
	case string:
		for _, layout := range []string{time.RFC3339Nano, time.RFC3339, "2006-01-02T15:04:05", "2006-01-02"} {
			if t, err := time.Parse(layout, val); err == nil {
				return t
			}
		}
	}
	return time.Time{}
}

func parseTime(value string) time.Time {
	if value == "" {
		return time.Time{}
	}
	for _, layout := range []string{time.RFC3339Nano, time.RFC3339, "2006-01-02T15:04:05", "2006-01-02"} {
		if t, err := time.Parse(layout, value); err == nil {
			return t
		}
	}
	return time.Time{}
}
