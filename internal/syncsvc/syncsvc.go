package syncsvc

import (
	"bytes"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"database/sql"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/gnur/exokephalos/internal/version"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/scanner"
	"gopkg.in/yaml.v3"
	_ "modernc.org/sqlite"
)

type Server struct {
	db              *sql.DB
	baseDir         string
	onConfigChanged func()
}

type Change struct {
	Op          string                 `json:"op"`
	TargetKind  string                 `json:"target_kind"`
	ID          string                 `json:"id"`
	Path        string                 `json:"path"`
	Frontmatter map[string]interface{} `json:"frontmatter,omitempty"`
	Body        string                 `json:"body,omitempty"`
	Content     string                 `json:"content,omitempty"`
}

type ChangeResponse struct {
	Revision int64 `json:"revision"`
}

type Client struct {
	ID         string `json:"id"`
	Label      string `json:"label"`
	Status     string `json:"status"`
	CreatedAt  string `json:"created_at"`
	ApprovedAt string `json:"approved_at"`
}

func NewServer(dbPath string) (*Server, error) {
	if err := os.MkdirAll(filepath.Dir(dbPath), 0755); err != nil {
		return nil, err
	}
	db, err := sql.Open("sqlite", dbPath)
	if err != nil {
		return nil, err
	}
	s := &Server{db: db, baseDir: "."}
	if err := s.migrate(); err != nil {
		_ = db.Close()
		return nil, err
	}
	return s, nil
}

func (s *Server) Close() error { return s.db.Close() }

func (s *Server) SetBaseDir(baseDir string) {
	if baseDir != "" {
		s.baseDir = baseDir
	}
}

func (s *Server) SetOnConfigChanged(cb func()) {
	s.onConfigChanged = cb
}

func (s *Server) DB() *sql.DB {
	return s.db
}

func (s *Server) LatestRevision() int64 {
	return s.latestRevision()
}

func (s *Server) ApplyChange(ch Change) (int64, error) {
	return s.applyChange(ch)
}

func (s *Server) Snapshot() ([]Change, []Change, error) {
	items, err := s.items()
	if err != nil {
		return nil, nil, err
	}
	configs, err := s.configs()
	if err != nil {
		return nil, nil, err
	}
	return items, configs, nil
}

func (s *Server) Register(mux *http.ServeMux) {
	s.RegisterAPI(mux)
}

func (s *Server) RegisterAPI(mux *http.ServeMux) {
	mux.HandleFunc("GET /api/sync/version", s.handleVersion)
	mux.HandleFunc("POST /api/sync/enroll", s.handleEnroll)
	mux.HandleFunc("GET /api/sync/enroll/status", s.handleEnrollStatus)
	mux.HandleFunc("POST /api/sync/changes", s.requireSignature(s.handleChanges))
	mux.HandleFunc("GET /api/sync/snapshot", s.requireSignature(s.handleSnapshot))
	mux.HandleFunc("GET /api/sync/events", s.requireSignature(s.handleEvents))
}

func (s *Server) handleVersion(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]string{"version": version.Version})
}

func (s *Server) RegisterWebEvents(mux *http.ServeMux) {
	mux.HandleFunc("GET /api/events", s.handleWebEvents)
}

func (s *Server) migrate() error {
	stmts := []string{
		`PRAGMA journal_mode = WAL`,
		`CREATE TABLE IF NOT EXISTS items (
			id TEXT PRIMARY KEY,
			path TEXT NOT NULL,
			frontmatter TEXT NOT NULL,
			body TEXT NOT NULL,
			type TEXT NOT NULL,
			tags TEXT NOT NULL,
			created TEXT NOT NULL,
			revision INTEGER NOT NULL,
			updated_at TEXT NOT NULL,
			deleted_at TEXT NOT NULL DEFAULT ''
		)`,
		`CREATE TABLE IF NOT EXISTS configs (
			path TEXT PRIMARY KEY,
			content TEXT NOT NULL,
			revision INTEGER NOT NULL,
			updated_at TEXT NOT NULL,
			deleted_at TEXT NOT NULL DEFAULT ''
		)`,
		`CREATE TABLE IF NOT EXISTS revisions (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			target_kind TEXT NOT NULL,
			target_id TEXT NOT NULL,
			op TEXT NOT NULL,
			created_at TEXT NOT NULL
		)`,
		`CREATE TABLE IF NOT EXISTS clients (
			id TEXT PRIMARY KEY,
			label TEXT NOT NULL,
			public_key TEXT NOT NULL,
			status TEXT NOT NULL,
			enrollment_token TEXT NOT NULL,
			created_at TEXT NOT NULL,
			approved_at TEXT NOT NULL DEFAULT ''
		)`,
		`CREATE TABLE IF NOT EXISTS nonces (
			client_id TEXT NOT NULL,
			nonce TEXT NOT NULL,
			created_at TEXT NOT NULL,
			PRIMARY KEY(client_id, nonce)
		)`,
	}
	for _, stmt := range stmts {
		if _, err := s.db.Exec(stmt); err != nil {
			return err
		}
	}
	return nil
}

func (s *Server) handleEnroll(w http.ResponseWriter, r *http.Request) {
	var req struct {
		ClientID  string `json:"client_id"`
		Label     string `json:"label"`
		PublicKey string `json:"public_key"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid json", http.StatusBadRequest)
		return
	}
	req.ClientID = strings.TrimSpace(req.ClientID)
	if req.ClientID == "" || req.PublicKey == "" {
		http.Error(w, "missing client_id or public_key", http.StatusBadRequest)
		return
	}
	if req.Label == "" {
		req.Label = req.ClientID
	}
	slog.Info("incoming sync enroll", "client_id", req.ClientID, "label", req.Label, "remote_addr", r.RemoteAddr)
	token := randomToken()
	now := time.Now().UTC().Format(time.RFC3339Nano)
	_, err := s.db.Exec(`INSERT INTO clients(id, label, public_key, status, enrollment_token, created_at) VALUES(?, ?, ?, 'pending', ?, ?)
		ON CONFLICT(id) DO UPDATE SET label = excluded.label, public_key = excluded.public_key, status = 'pending', enrollment_token = excluded.enrollment_token`,
		req.ClientID, req.Label, req.PublicKey, token, now)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	s.recordRevision("client", req.ClientID, "enroll_client")
	slog.Info("sync client enrollment pending", "client_id", req.ClientID, "label", req.Label)
	writeJSON(w, map[string]string{"status": "pending", "enrollment_token": token})
}

func (s *Server) handleEnrollStatus(w http.ResponseWriter, r *http.Request) {
	clientID := r.URL.Query().Get("client_id")
	token := r.URL.Query().Get("token")
	var status string
	err := s.db.QueryRow(`SELECT status FROM clients WHERE id = ? AND enrollment_token = ?`, clientID, token).Scan(&status)
	if err == sql.ErrNoRows {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	slog.Info("outgoing sync enrollment status", "client_id", clientID, "status", status)
	writeJSON(w, map[string]string{"status": status})
}

func (s *Server) handleChanges(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Changes []Change `json:"changes"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid json", http.StatusBadRequest)
		return
	}
	clientID := r.Header.Get("X-Exo-Client-ID")
	slog.Info("incoming sync changes", "client_id", clientID, "count", len(req.Changes), "remote_addr", r.RemoteAddr)
	var lastRev int64
	for _, ch := range req.Changes {
		rev, err := s.applyChange(ch)
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		lastRev = rev
	}
	slog.Info("outgoing sync changes response", "client_id", clientID, "revision", lastRev, "count", len(req.Changes))
	writeJSON(w, ChangeResponse{Revision: lastRev})
}

func (s *Server) applyChange(ch Change) (int64, error) {
	if ch.TargetKind == "" {
		ch.TargetKind = "item"
	}
	tx, err := s.db.Begin()
	if err != nil {
		return 0, err
	}
	defer tx.Rollback()
	now := time.Now().UTC().Format(time.RFC3339Nano)
	res, err := tx.Exec(`INSERT INTO revisions(target_kind, target_id, op, created_at) VALUES(?, ?, ?, ?)`, ch.TargetKind, firstNonEmpty(ch.ID, ch.Path), ch.Op, now)
	if err != nil {
		return 0, err
	}
	rev, _ := res.LastInsertId()
	var createdNote bool
	var createdNoteType string
	var createdNotePath string
	switch ch.TargetKind {
	case "config":
		if ch.Op == "delete_config" || ch.Op == "delete" {
			_, err = tx.Exec(`UPDATE configs SET deleted_at = ?, revision = ? WHERE path = ?`, now, rev, ch.Path)
		} else {
			_, err = tx.Exec(`INSERT INTO configs(path, content, revision, updated_at, deleted_at) VALUES(?, ?, ?, ?, '')
				ON CONFLICT(path) DO UPDATE SET content = excluded.content, revision = excluded.revision, updated_at = excluded.updated_at, deleted_at = ''`,
				ch.Path, ch.Content, rev, now)
		}
	default:
		if ch.ID == "" {
			return 0, fmt.Errorf("missing item id")
		}
		var existingID string
		exists := tx.QueryRow(`SELECT id FROM items WHERE id = ? AND deleted_at = ''`, ch.ID).Scan(&existingID) == nil
		if ch.Op == "delete_item" || ch.Op == "delete" {
			_, err = tx.Exec(`UPDATE items SET deleted_at = ?, revision = ? WHERE id = ?`, now, rev, ch.ID)
		} else {
			typ := markdown.FMString(ch.Frontmatter, "type")
			tags, _ := json.Marshal(markdown.ExtractTags(ch.Frontmatter))
			created := fmt.Sprint(ch.Frontmatter["created"])
			fm, _ := json.Marshal(ch.Frontmatter)
			_, err = tx.Exec(`INSERT INTO items(id, path, frontmatter, body, type, tags, created, revision, updated_at, deleted_at) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, '')
				ON CONFLICT(id) DO UPDATE SET path = excluded.path, frontmatter = excluded.frontmatter, body = excluded.body, type = excluded.type, tags = excluded.tags, created = excluded.created, revision = excluded.revision, updated_at = excluded.updated_at, deleted_at = ''`,
				ch.ID, ch.Path, string(fm), ch.Body, typ, string(tags), created, rev, now)
			if err == nil && !exists {
				createdNote = true
				createdNoteType = typ
				createdNotePath = ch.Path
			}
		}
	}
	if err != nil {
		return 0, err
	}
	if err := tx.Commit(); err != nil {
		return 0, err
	}
	if createdNote {
		slog.Info("note created", "id", ch.ID, "path", createdNotePath, "type", createdNoteType, "revision", rev)
	}
	if ch.TargetKind == "config" && s.onConfigChanged != nil {
		s.onConfigChanged()
	}
	return rev, nil
}

func (s *Server) handleSnapshot(w http.ResponseWriter, r *http.Request) {
	items, err := s.items()
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	configs, err := s.configs()
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	slog.Info("outgoing sync snapshot", "client_id", r.Header.Get("X-Exo-Client-ID"), "items", len(items), "configs", len(configs))
	writeJSON(w, map[string]interface{}{"items": items, "configs": configs})
}

func (s *Server) handleEvents(w http.ResponseWriter, r *http.Request) {
	since, _ := strconv.ParseInt(r.URL.Query().Get("since_revision"), 10, 64)
	s.streamEvents(w, r, since)
}

func (s *Server) handleWebEvents(w http.ResponseWriter, r *http.Request) {
	since, err := strconv.ParseInt(r.URL.Query().Get("since_revision"), 10, 64)
	if err != nil || r.URL.Query().Get("since_revision") == "" {
		since = s.latestRevision()
	}
	s.streamEvents(w, r, since)
}

func (s *Server) streamEvents(w http.ResponseWriter, r *http.Request, since int64) {
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	flusher, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "streaming unsupported", http.StatusInternalServerError)
		return
	}
	ticker := time.NewTicker(2 * time.Second)
	defer ticker.Stop()
	fmt.Fprint(w, ": connected\n\n")
	flusher.Flush()
	for {
		latest, err := s.writeEventsAfter(w, since)
		if err != nil {
			fmt.Fprintf(w, "event: error\ndata: %q\n\n", err.Error())
			flusher.Flush()
			return
		}
		if latest > since {
			since = latest
			flusher.Flush()
		}
		select {
		case <-r.Context().Done():
			return
		case <-ticker.C:
			fmt.Fprint(w, ": ping\n\n")
			flusher.Flush()
		}
	}
}

func (s *Server) latestRevision() int64 {
	var rev int64
	_ = s.db.QueryRow(`SELECT COALESCE(MAX(id), 0) FROM revisions`).Scan(&rev)
	return rev
}

func (s *Server) writeEventsAfter(w io.Writer, since int64) (int64, error) {
	rows, err := s.db.Query(`SELECT id, target_kind, target_id, op, created_at FROM revisions WHERE id > ? ORDER BY id ASC`, since)
	if err != nil {
		return since, err
	}
	defer rows.Close()
	latest := since
	for rows.Next() {
		var rev int64
		var kind, target, op, created string
		if err := rows.Scan(&rev, &kind, &target, &op, &created); err != nil {
			return latest, err
		}
		b, _ := json.Marshal(map[string]interface{}{"revision": rev, "target_kind": kind, "target_id": target, "op": op, "created_at": created})
		fmt.Fprintf(w, "event: change\ndata: %s\n\n", b)
		slog.Info("outgoing sync event", "revision", rev, "target_kind", kind, "target_id", target, "op", op)
		latest = rev
	}
	return latest, rows.Err()
}

func (s *Server) Clients() ([]Client, error) {
	rows, err := s.db.Query(`SELECT id, label, status, created_at, approved_at FROM clients ORDER BY created_at DESC`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var clients []Client
	for rows.Next() {
		var c Client
		if err := rows.Scan(&c.ID, &c.Label, &c.Status, &c.CreatedAt, &c.ApprovedAt); err != nil {
			return nil, err
		}
		clients = append(clients, c)
	}
	return clients, rows.Err()
}

func (s *Server) ApproveClient(clientID string) error {
	if _, err := s.db.Exec(`UPDATE clients SET status = 'approved', approved_at = ? WHERE id = ?`, time.Now().UTC().Format(time.RFC3339Nano), clientID); err != nil {
		return err
	}
	s.recordRevision("client", clientID, "approve_client")
	return nil
}

func (s *Server) RevokeClient(clientID string) error {
	if _, err := s.db.Exec(`UPDATE clients SET status = 'revoked' WHERE id = ?`, clientID); err != nil {
		return err
	}
	s.recordRevision("client", clientID, "revoke_client")
	return nil
}

func (s *Server) recordRevision(targetKind, targetID, op string) {
	_, _ = s.db.Exec(`INSERT INTO revisions(target_kind, target_id, op, created_at) VALUES(?, ?, ?, ?)`, targetKind, targetID, op, time.Now().UTC().Format(time.RFC3339Nano))
}

func (s *Server) requireSignature(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "reading body", http.StatusBadRequest)
			return
		}
		r.Body = io.NopCloser(bytes.NewReader(body))
		clientID := r.Header.Get("X-Exo-Client-ID")
		timestamp := r.Header.Get("X-Exo-Timestamp")
		nonce := r.Header.Get("X-Exo-Nonce")
		sigB64 := r.Header.Get("X-Exo-Signature")
		if clientID == "" || timestamp == "" || nonce == "" || sigB64 == "" {
			http.Error(w, "missing signature headers", http.StatusUnauthorized)
			return
		}
		ts, err := time.Parse(time.RFC3339Nano, timestamp)
		if err != nil || time.Since(ts) > 5*time.Minute || time.Until(ts) > 5*time.Minute {
			http.Error(w, "stale timestamp", http.StatusUnauthorized)
			return
		}
		var pubB64, status string
		err = s.db.QueryRow(`SELECT public_key, status FROM clients WHERE id = ?`, clientID).Scan(&pubB64, &status)
		if err != nil || status != "approved" {
			http.Error(w, "client not approved", http.StatusUnauthorized)
			return
		}
		// Prune nonces older than 5 minutes to prevent infinite table growth
		pruneTime := time.Now().Add(-5 * time.Minute).UTC().Format(time.RFC3339Nano)
		_, _ = s.db.Exec(`DELETE FROM nonces WHERE created_at < ?`, pruneTime)

		if _, err := s.db.Exec(`INSERT INTO nonces(client_id, nonce, created_at) VALUES(?, ?, ?)`, clientID, nonce, timestamp); err != nil {
			http.Error(w, "reused nonce", http.StatusUnauthorized)
			return
		}
		pub, err := base64.StdEncoding.DecodeString(pubB64)
		if err != nil || len(pub) != ed25519.PublicKeySize {
			http.Error(w, "invalid public key", http.StatusUnauthorized)
			return
		}
		sig, err := base64.StdEncoding.DecodeString(sigB64)
		if err != nil {
			http.Error(w, "invalid signature", http.StatusUnauthorized)
			return
		}
		bodyHash := sha256.Sum256(body)
		msg := strings.Join([]string{r.Method, r.URL.RequestURI(), timestamp, nonce, hex.EncodeToString(bodyHash[:])}, "\n")
		if !ed25519.Verify(ed25519.PublicKey(pub), []byte(msg), sig) {
			http.Error(w, "invalid signature", http.StatusUnauthorized)
			return
		}
		next(w, r)
	}
}

func (s *Server) items() ([]Change, error) {
	rows, err := s.db.Query(`SELECT id, path, frontmatter, body FROM items WHERE deleted_at = '' ORDER BY updated_at DESC`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var changes []Change
	for rows.Next() {
		var ch Change
		var fmJSON string
		if err := rows.Scan(&ch.ID, &ch.Path, &fmJSON, &ch.Body); err != nil {
			return nil, err
		}
		ch.Op = "upsert_item"
		ch.TargetKind = "item"
		_ = json.Unmarshal([]byte(fmJSON), &ch.Frontmatter)
		changes = append(changes, ch)
	}
	return changes, rows.Err()
}

func (s *Server) All() ([]scanner.Item, error) {
	changes, err := s.items()
	if err != nil {
		return nil, err
	}
	items := make([]scanner.Item, 0, len(changes))
	for _, ch := range changes {
		items = append(items, scanner.Item{
			Path:        s.pathForWeb(ch.Path),
			Frontmatter: ch.Frontmatter,
			Body:        ch.Body,
			ID:          ch.ID,
			Type:        markdown.FMString(ch.Frontmatter, "type"),
			Tags:        markdown.ExtractTags(ch.Frontmatter),
		})
	}
	return items, nil
}

func (s *Server) GetByID(id string) (*scanner.Item, error) {
	var ch Change
	var fmJSON string
	err := s.db.QueryRow(`SELECT id, path, frontmatter, body FROM items WHERE lower(id) = lower(?) AND deleted_at = ''`, id).Scan(&ch.ID, &ch.Path, &fmJSON, &ch.Body)
	if err != nil {
		return nil, err
	}
	_ = json.Unmarshal([]byte(fmJSON), &ch.Frontmatter)
	item := scanner.Item{
		Path:        s.pathForWeb(ch.Path),
		Frontmatter: ch.Frontmatter,
		Body:        ch.Body,
		ID:          ch.ID,
		Type:        markdown.FMString(ch.Frontmatter, "type"),
		Tags:        markdown.ExtractTags(ch.Frontmatter),
	}
	return &item, nil
}

func (s *Server) ReadRaw(path string) (string, error) {
	rel := s.relPath(path)
	var fmJSON, body string
	if err := s.db.QueryRow(`SELECT frontmatter, body FROM items WHERE path = ? AND deleted_at = ''`, rel).Scan(&fmJSON, &body); err != nil {
		return "", err
	}
	var fm map[string]interface{}
	_ = json.Unmarshal([]byte(fmJSON), &fm)
	return renderRawMarkdown(fm, body)
}

func (s *Server) WriteRaw(path, content string) error {
	fm, body, err := markdown.ParseFrontmatterBytes([]byte(content))
	if err != nil {
		return err
	}
	if fm == nil {
		fm = map[string]interface{}{}
	}
	return s.upsertFromWeb(path, fm, body)
}

func (s *Server) CreateItem(path string, fm map[string]interface{}, body string) error {
	return s.upsertFromWeb(path, fm, body)
}

func (s *Server) UpdateItem(path string, fm map[string]interface{}, body string) error {
	return s.upsertFromWeb(path, fm, body)
}

func (s *Server) DeleteItem(path string) error {
	rel := s.relPath(path)
	var id string
	if err := s.db.QueryRow(`SELECT id FROM items WHERE path = ? AND deleted_at = ''`, rel).Scan(&id); err != nil {
		return err
	}
	_, err := s.applyChange(Change{Op: "delete_item", TargetKind: "item", ID: id, Path: rel})
	return err
}

func (s *Server) upsertFromWeb(path string, fm map[string]interface{}, body string) error {
	rel := s.relPath(path)
	idVal := markdown.FMString(fm, "id")
	if idVal == "" {
		return fmt.Errorf("missing item id")
	}
	_, err := s.applyChange(Change{
		Op:          "upsert_item",
		TargetKind:  "item",
		ID:          idVal,
		Path:        rel,
		Frontmatter: fm,
		Body:        body,
	})
	return err
}

func (s *Server) relPath(path string) string {
	if path == "" {
		return path
	}
	clean := filepath.Clean(path)
	var rel string
	if filepath.IsAbs(clean) {
		var err error
		rel, err = filepath.Rel(s.baseDir, clean)
		if err != nil || strings.HasPrefix(rel, "..") {
			return ""
		}
	} else {
		rel = clean
		if strings.HasPrefix(rel, "..") {
			return ""
		}
	}
	return filepath.ToSlash(rel)
}

func (s *Server) pathForWeb(path string) string {
	if filepath.IsAbs(path) {
		return path
	}
	return filepath.Join(s.baseDir, filepath.FromSlash(path))
}

func renderRawMarkdown(fm map[string]interface{}, body string) (string, error) {
	var buf bytes.Buffer
	buf.WriteString("---\n")
	enc := yaml.NewEncoder(&buf)
	enc.SetIndent(2)
	if err := enc.Encode(fm); err != nil {
		return "", err
	}
	buf.WriteString("---\n")
	buf.WriteString(body)
	return buf.String(), nil
}

func (s *Server) configs() ([]Change, error) {
	rows, err := s.db.Query(`SELECT path, content FROM configs WHERE deleted_at = '' ORDER BY path ASC`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var changes []Change
	for rows.Next() {
		var ch Change
		if err := rows.Scan(&ch.Path, &ch.Content); err != nil {
			return nil, err
		}
		ch.Op = "upsert_config"
		ch.TargetKind = "config"
		changes = append(changes, ch)
	}
	return changes, rows.Err()
}

func (s *Server) Configs() ([]Change, error) {
	return s.configs()
}

func (s *Server) UpsertConfig(path, content string) (int64, error) {
	return s.applyChange(Change{Op: "upsert_config", TargetKind: "config", Path: path, Content: content})
}

func LoadConfigFromServerDB(dbPath string) (*config.Config, error) {
	db, err := sql.Open("sqlite", dbPath)
	if err != nil {
		return nil, err
	}
	defer db.Close()
	return LoadConfigFromDB(db)
}

func LoadConfigFromDB(db *sql.DB) (*config.Config, error) {
	rows, err := db.Query(`SELECT path, content FROM configs WHERE deleted_at = '' ORDER BY path ASC`)
	if err != nil {
		return &config.Config{Views: map[string]config.ViewConfig{}, Actions: map[string]config.ActionConfig{}}, nil
	}
	defer rows.Close()

	var contents []config.NamedContent
	for rows.Next() {
		var path string
		var content string
		if err := rows.Scan(&path, &content); err != nil {
			return nil, err
		}
		contents = append(contents, config.NamedContent{Name: path, Content: []byte(content)})
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	if len(contents) == 0 {
		return &config.Config{Views: map[string]config.ViewConfig{}, Actions: map[string]config.ActionConfig{}}, nil
	}
	return config.LoadContents(contents)
}

func (s *Server) LoadConfig() (*config.Config, error) {
	return LoadConfigFromDB(s.db)
}

func BuildLocalChanges(baseDir string, c *cache.Cache, includeAll bool) ([]Change, error) {
	if err := c.Sync(); err != nil {
		return nil, err
	}
	items, err := c.All()
	if err != nil {
		return nil, err
	}
	var changes []Change
	for _, item := range items {
		rel, _ := filepath.Rel(baseDir, item.Path)
		changes = append(changes, Change{
			Op:          "upsert_item",
			TargetKind:  "item",
			ID:          item.ID,
			Path:        rel,
			Frontmatter: item.Frontmatter,
			Body:        item.Body,
		})
	}
	configs, err := filepath.Glob(filepath.Join(baseDir, "*.toml"))
	if err == nil {
		for _, path := range configs {
			data, err := os.ReadFile(path)
			if err != nil {
				continue
			}
			rel, _ := filepath.Rel(baseDir, path)
			changes = append(changes, Change{Op: "upsert_config", TargetKind: "config", Path: rel, Content: string(data)})
		}
	}
	return changes, nil
}

func EnsureKeypair(path string) (ed25519.PublicKey, ed25519.PrivateKey, error) {
	if data, err := os.ReadFile(path); err == nil {
		priv, err := base64.StdEncoding.DecodeString(strings.TrimSpace(string(data)))
		if err != nil {
			return nil, nil, err
		}
		if len(priv) != ed25519.PrivateKeySize {
			return nil, nil, fmt.Errorf("invalid private key size")
		}
		return ed25519.PrivateKey(priv).Public().(ed25519.PublicKey), ed25519.PrivateKey(priv), nil
	}
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return nil, nil, err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		return nil, nil, err
	}
	if err := os.WriteFile(path, []byte(base64.StdEncoding.EncodeToString(priv)), 0600); err != nil {
		return nil, nil, err
	}
	return pub, priv, nil
}

func SignRequest(req *http.Request, body []byte, clientID string, priv ed25519.PrivateKey) {
	timestamp := time.Now().UTC().Format(time.RFC3339Nano)
	nonce := randomToken()
	bodyHash := sha256.Sum256(body)
	msg := strings.Join([]string{req.Method, req.URL.RequestURI(), timestamp, nonce, hex.EncodeToString(bodyHash[:])}, "\n")
	sig := ed25519.Sign(priv, []byte(msg))
	req.Header.Set("X-Exo-Client-ID", clientID)
	req.Header.Set("X-Exo-Timestamp", timestamp)
	req.Header.Set("X-Exo-Nonce", nonce)
	req.Header.Set("X-Exo-Signature", base64.StdEncoding.EncodeToString(sig))
}

func EnqueueChanges(c *cache.Cache, changes []Change) error {
	for _, ch := range changes {
		payload, _ := json.Marshal(ch)
		target := ch.ID
		if target == "" {
			target = ch.Path
		}
		if err := c.EnqueueOutbox(ch.Op, ch.TargetKind, target, ch.Path, string(payload)); err != nil {
			return err
		}
	}
	return nil
}

func randomToken() string {
	var b [18]byte
	_, _ = rand.Read(b[:])
	return base64.RawURLEncoding.EncodeToString(b[:])
}

func firstNonEmpty(values ...string) string {
	for _, value := range values {
		if value != "" {
			return value
		}
	}
	return ""
}

func writeJSON(w http.ResponseWriter, v interface{}) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(v)
}

func ItemFromMarkdown(path string, content []byte) (*scanner.Item, error) {
	fm, body, err := markdown.ParseFrontmatterBytes(content)
	if err != nil {
		return nil, err
	}
	return &scanner.Item{
		Path:        path,
		ID:          markdown.FMString(fm, "id"),
		Type:        markdown.FMString(fm, "type"),
		Tags:        markdown.ExtractTags(fm),
		Created:     time.Now(),
		Frontmatter: fm,
		Body:        body,
	}, nil
}
