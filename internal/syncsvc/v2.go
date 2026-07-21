package syncsvc

// The v2 protocol is deliberately independent from the old revision/snapshot
// protocol.  Keeping its state in separate tables makes an epoch cut-over
// atomic and, importantly, prevents a v1 client from accidentally reviving a
// record which v2 has tombstoned.

import (
	"crypto/rand"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"strconv"
	"strings"
	"time"
)

const SyncProtocolVersion = 2

type HLC struct {
	PhysicalMS int64  `json:"physical_ms"`
	Logical    int64  `json:"logical"`
	ActorID    string `json:"actor_id"`
}

func (v HLC) valid() bool { return v.PhysicalMS > 0 && v.Logical >= 0 && v.ActorID != "" }
func (v HLC) compare(other HLC) int {
	if v.PhysicalMS != other.PhysicalMS {
		if v.PhysicalMS < other.PhysicalMS {
			return -1
		}
		return 1
	}
	if v.Logical != other.Logical {
		if v.Logical < other.Logical {
			return -1
		}
		return 1
	}
	return strings.Compare(v.ActorID, other.ActorID)
}

type SyncOperation struct {
	ID          string                 `json:"id"`
	Epoch       string                 `json:"epoch"`
	ActorID     string                 `json:"actor_id"`
	Kind        string                 `json:"kind"`
	Target      string                 `json:"target"`
	Delete      bool                   `json:"delete"`
	Path        string                 `json:"path,omitempty"`
	Version     HLC                    `json:"version"`
	Frontmatter map[string]interface{} `json:"frontmatter,omitempty"`
	Body        string                 `json:"body,omitempty"`
	Content     string                 `json:"content,omitempty"`
	Hash        string                 `json:"hash,omitempty"`
	MIME        string                 `json:"mime,omitempty"`
	Size        int64                  `json:"size,omitempty"`
}

type OperationResult struct {
	ID     string `json:"id"`
	Status string `json:"status"`
	Error  string `json:"error,omitempty"`
	Cursor int64  `json:"cursor,omitempty"`
}

func (s *Server) migrateV2() error {
	for _, q := range []string{
		`CREATE TABLE IF NOT EXISTS sync_epoch (id INTEGER PRIMARY KEY CHECK(id=1), epoch TEXT NOT NULL, created_at TEXT NOT NULL)`,
		`CREATE TABLE IF NOT EXISTS sync_records (kind TEXT NOT NULL, target TEXT NOT NULL, operation TEXT NOT NULL, path TEXT NOT NULL, payload TEXT NOT NULL, deleted INTEGER NOT NULL, physical_ms INTEGER NOT NULL, logical INTEGER NOT NULL, actor_id TEXT NOT NULL, cursor INTEGER NOT NULL, PRIMARY KEY(kind,target))`,
		`CREATE TABLE IF NOT EXISTS sync_feed (cursor INTEGER PRIMARY KEY AUTOINCREMENT, operation TEXT NOT NULL, kind TEXT NOT NULL, target TEXT NOT NULL, created_at TEXT NOT NULL)`,
		`CREATE TABLE IF NOT EXISTS sync_operations (id TEXT PRIMARY KEY, status TEXT NOT NULL, cursor INTEGER NOT NULL DEFAULT 0, error TEXT NOT NULL DEFAULT '')`,
		`CREATE TABLE IF NOT EXISTS sync_acks (actor_id TEXT PRIMARY KEY, cursor INTEGER NOT NULL DEFAULT 0, retired_at TEXT NOT NULL DEFAULT '')`,
		`CREATE TABLE IF NOT EXISTS sync_devices (id TEXT PRIMARY KEY, label TEXT NOT NULL, kind TEXT NOT NULL, created_at TEXT NOT NULL, retired_at TEXT NOT NULL DEFAULT '')`,
	} {
		if _, err := s.db.Exec(q); err != nil {
			return err
		}
	}
	return nil
}

func (s *Server) Epoch() (string, error) {
	var epoch string
	err := s.db.QueryRow(`SELECT epoch FROM sync_epoch WHERE id=1`).Scan(&epoch)
	if err == nil {
		return epoch, nil
	}
	if err != sql.ErrNoRows {
		return "", err
	}
	b := make([]byte, 16)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	epoch = hex.EncodeToString(b)
	_, err = s.db.Exec(`INSERT INTO sync_epoch(id,epoch,created_at) VALUES(1,?,?)`, epoch, time.Now().UTC().Format(time.RFC3339Nano))
	return epoch, err
}

func (s *Server) PushOperations(actor string, ops []SyncOperation) ([]OperationResult, error) {
	epoch, err := s.Epoch()
	if err != nil {
		return nil, err
	}
	results := make([]OperationResult, 0, len(ops))
	for _, op := range ops {
		if op.ID == "" || op.Epoch != epoch || op.ActorID != actor || !op.Version.valid() || op.Version.ActorID != actor || !validTarget(op) {
			results = append(results, OperationResult{ID: op.ID, Status: "rejected", Error: "invalid sync operation"})
			continue
		}
		var status, failure string
		var cursor int64
		err := s.db.QueryRow(`SELECT status,cursor,error FROM sync_operations WHERE id=?`, op.ID).Scan(&status, &cursor, &failure)
		if err == nil {
			results = append(results, OperationResult{ID: op.ID, Status: status, Cursor: cursor, Error: failure})
			continue
		}
		if err != sql.ErrNoRows {
			return nil, err
		}
		payload, _ := json.Marshal(op)
		tx, err := s.db.Begin()
		if err != nil {
			return nil, err
		}
		var p, l int64
		var a string
		err = tx.QueryRow(`SELECT physical_ms,logical,actor_id FROM sync_records WHERE kind=? AND target=?`, op.Kind, op.Target).Scan(&p, &l, &a)
		if err == nil && op.Version.compare(HLC{p, l, a}) <= 0 {
			_, err = tx.Exec(`INSERT INTO sync_operations(id,status) VALUES(?, 'superseded')`, op.ID)
			if err == nil {
				err = tx.Commit()
			} else {
				_ = tx.Rollback()
			}
			if err != nil {
				return nil, err
			}
			results = append(results, OperationResult{ID: op.ID, Status: "superseded"})
			continue
		}
		if err != nil && err != sql.ErrNoRows {
			_ = tx.Rollback()
			return nil, err
		}
		res, err := tx.Exec(`INSERT INTO sync_feed(operation,kind,target,created_at) VALUES(?,?,?,?)`, op.ID, op.Kind, op.Target, time.Now().UTC().Format(time.RFC3339Nano))
		if err != nil {
			_ = tx.Rollback()
			return nil, err
		}
		cursor, _ = res.LastInsertId()
		_, err = tx.Exec(`INSERT INTO sync_records(kind,target,operation,path,payload,deleted,physical_ms,logical,actor_id,cursor) VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT(kind,target) DO UPDATE SET operation=excluded.operation,path=excluded.path,payload=excluded.payload,deleted=excluded.deleted,physical_ms=excluded.physical_ms,logical=excluded.logical,actor_id=excluded.actor_id,cursor=excluded.cursor`, op.Kind, op.Target, op.ID, op.Path, string(payload), boolInt(op.Delete), op.Version.PhysicalMS, op.Version.Logical, op.Version.ActorID, cursor)
		if err == nil {
			_, err = tx.Exec(`INSERT INTO sync_operations(id,status,cursor) VALUES(?, 'applied', ?)`, op.ID, cursor)
		}
		if err == nil {
			err = tx.Commit()
		} else {
			_ = tx.Rollback()
		}
		if err != nil {
			return nil, err
		}
		// Maintain the existing SQLite-backed web projection while callers move
		// to v2 reads.  The v2 record remains the ordering authority.
		if err := s.applyOperationProjection(op); err != nil {
			return nil, err
		}
		results = append(results, OperationResult{ID: op.ID, Status: "applied", Cursor: cursor})
	}
	return results, nil
}

func (s *Server) applyOperationProjection(op SyncOperation) error {
	change := Change{Path: op.Path}
	switch op.Kind {
	case "item":
		change.TargetKind, change.ID, change.Frontmatter, change.Body = "item", op.Target, op.Frontmatter, op.Body
		if op.Delete {
			change.Op = "delete_item"
		} else {
			change.Op = "upsert_item"
		}
	case "config":
		change.TargetKind, change.Path, change.Content = "config", op.Target, op.Content
		if op.Delete {
			change.Op = "delete_config"
		} else {
			change.Op = "upsert_config"
		}
	case "asset":
		change.TargetKind, change.Path, change.Hash, change.MIME, change.Size = "asset", op.Target, op.Hash, op.MIME, op.Size
		if op.Delete {
			change.Op = "delete_asset"
		} else {
			change.Op = "upsert_asset"
		}
	}
	_, err := s.applyChange(change)
	return err
}
func validTarget(op SyncOperation) bool {
	return (op.Kind == "item" || op.Kind == "config" || op.Kind == "asset") && op.Target != ""
}
func boolInt(v bool) int {
	if v {
		return 1
	}
	return 0
}

func (s *Server) PullOperations(after int64, limit int) ([]SyncOperation, int64, error) {
	if limit <= 0 || limit > 1000 {
		limit = 500
	}
	rows, err := s.db.Query(`SELECT r.payload,f.cursor FROM sync_feed f JOIN sync_records r ON r.operation=f.operation WHERE f.cursor>? ORDER BY f.cursor LIMIT ?`, after, limit)
	if err != nil {
		return nil, after, err
	}
	defer rows.Close()
	var out []SyncOperation
	cursor := after
	for rows.Next() {
		var raw string
		if err = rows.Scan(&raw, &cursor); err != nil {
			return nil, after, err
		}
		var op SyncOperation
		if err = json.Unmarshal([]byte(raw), &op); err != nil {
			return nil, after, err
		}
		out = append(out, op)
	}
	return out, cursor, rows.Err()
}
func (s *Server) Acknowledge(actor string, cursor int64) error {
	_, err := s.db.Exec(`INSERT INTO sync_acks(actor_id,cursor) VALUES(?,?) ON CONFLICT(actor_id) DO UPDATE SET cursor=MAX(cursor,excluded.cursor)`, actor, cursor)
	return err
}

// RetireDevice stops a lost device from holding tombstones forever. It is
// intentionally explicit: revoking credentials is not equivalent to saying a
// projection will never reconnect.
func (s *Server) RetireDevice(actor string) error {
	_, err := s.db.Exec(`INSERT INTO sync_acks(actor_id,retired_at) VALUES(?,?) ON CONFLICT(actor_id) DO UPDATE SET retired_at=excluded.retired_at`, actor, time.Now().UTC().Format(time.RFC3339Nano))
	return err
}

// CompactTombstones deletes only records whose feed cursor is known to every
// active device.  Returning the count makes this suitable for an admin UI.
func (s *Server) CompactTombstones() (int64, error) {
	var floor sql.NullInt64
	if err := s.db.QueryRow(`SELECT MIN(cursor) FROM sync_acks WHERE retired_at=''`).Scan(&floor); err != nil {
		return 0, err
	}
	if !floor.Valid {
		return 0, nil
	}
	res, err := s.db.Exec(`DELETE FROM sync_records WHERE deleted=1 AND cursor<=?`, floor.Int64)
	if err != nil {
		return 0, err
	}
	return res.RowsAffected()
}

func (s *Server) handleV2Bootstrap(w http.ResponseWriter, r *http.Request) {
	epoch, err := s.Epoch()
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	ops, cursor, err := s.PullOperations(0, 1000)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	writeJSON(w, map[string]interface{}{"protocol": SyncProtocolVersion, "epoch": epoch, "cursor": cursor, "operations": ops})
}
func (s *Server) handleV2Push(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Operations []SyncOperation `json:"operations"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid json", 400)
		return
	}
	out, err := s.PushOperations(r.Header.Get("X-Exo-Client-ID"), req.Operations)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	writeJSON(w, map[string]interface{}{"results": out})
}
func (s *Server) handleV2Pull(w http.ResponseWriter, r *http.Request) {
	after, _ := strconv.ParseInt(r.URL.Query().Get("cursor"), 10, 64)
	ops, cursor, err := s.PullOperations(after, 0)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	writeJSON(w, map[string]interface{}{"cursor": cursor, "operations": ops})
}
func (s *Server) handleV2Ack(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Cursor int64 `json:"cursor"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid json", 400)
		return
	}
	if err := s.Acknowledge(r.Header.Get("X-Exo-Client-ID"), req.Cursor); err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) handleV2Retire(w http.ResponseWriter, r *http.Request) {
	var req struct {
		ActorID string `json:"actor_id"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || req.ActorID == "" {
		http.Error(w, "invalid json", http.StatusBadRequest)
		return
	}
	if err := s.RetireDevice(req.ActorID); err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
