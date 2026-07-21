package syncsvc

import (
	"bytes"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/gnur/exokephalos/internal/version"
)

func TestV2OperationsUseDeterministicLWWAndAreIdempotent(t *testing.T) {
	s, err := NewServer(filepath.Join(t.TempDir(), "server.sqlite"))
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	epoch, err := s.Epoch()
	if err != nil {
		t.Fatal(err)
	}
	newer := SyncOperation{ID: "op-new", Epoch: epoch, ActorID: "b", Kind: "item", Target: "note-1", Path: "note.md", Version: HLC{PhysicalMS: 100, Logical: 0, ActorID: "b"}, Frontmatter: map[string]interface{}{"id": "note-1"}, Body: "new"}
	result, err := s.PushOperations("b", []SyncOperation{newer})
	if err != nil {
		t.Fatal(err)
	}
	if result[0].Status != "applied" {
		t.Fatalf("new result = %+v", result[0])
	}
	// Equal clocks resolve on actor ID, so this later-arriving operation loses.
	older := newer
	older.ID = "op-old"
	older.ActorID = "a"
	older.Version.ActorID = "a"
	older.Body = "old"
	result, err = s.PushOperations("a", []SyncOperation{older})
	if err != nil {
		t.Fatal(err)
	}
	if result[0].Status != "superseded" {
		t.Fatalf("old result = %+v", result[0])
	}
	result, err = s.PushOperations("b", []SyncOperation{newer})
	if err != nil {
		t.Fatal(err)
	}
	if result[0].Status != "applied" || result[0].Cursor == 0 {
		t.Fatalf("retry result = %+v", result[0])
	}
	ops, cursor, err := s.PullOperations(0, 10)
	if err != nil {
		t.Fatal(err)
	}
	if cursor != result[0].Cursor || len(ops) != 1 || ops[0].Body != "new" {
		t.Fatalf("pull = %#v cursor=%d", ops, cursor)
	}
	if err := s.Acknowledge("b", cursor); err != nil {
		t.Fatal(err)
	}
	var ack int64
	if err := s.DB().QueryRow(`SELECT cursor FROM sync_acks WHERE actor_id='b'`).Scan(&ack); err != nil || ack != cursor {
		t.Fatalf("ack = %d, %v", ack, err)
	}
}

func TestV2RejectsWrongEpochWithoutBlockingOtherOperations(t *testing.T) {
	s, err := NewServer(filepath.Join(t.TempDir(), "server.sqlite"))
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	epoch, _ := s.Epoch()
	valid := SyncOperation{ID: "ok", Epoch: epoch, ActorID: "device", Kind: "config", Target: "exo.fnl", Version: HLC{PhysicalMS: time.Now().UnixMilli(), ActorID: "device"}, Content: "{}"}
	invalid := valid
	invalid.ID = "bad"
	invalid.Epoch = "wrong"
	result, err := s.PushOperations("device", []SyncOperation{invalid, valid})
	if err != nil {
		t.Fatal(err)
	}
	if result[0].Status != "rejected" || result[1].Status != "applied" {
		t.Fatalf("results = %+v", result)
	}
}

func TestV2TombstoneCompactsOnlyAfterEveryActiveAcknowledgement(t *testing.T) {
	s, err := NewServer(filepath.Join(t.TempDir(), "server.sqlite"))
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	epoch, _ := s.Epoch()
	upsert := SyncOperation{ID: "create", Epoch: epoch, ActorID: "a", Kind: "item", Target: "n", Path: "n.md", Version: HLC{PhysicalMS: 1, ActorID: "a"}, Frontmatter: map[string]interface{}{"id": "n", "type": "note"}}
	if _, err := s.PushOperations("a", []SyncOperation{upsert}); err != nil {
		t.Fatal(err)
	}
	deleteOp := upsert
	deleteOp.ID = "delete"
	deleteOp.Delete = true
	deleteOp.Version = HLC{PhysicalMS: 2, ActorID: "a"}
	result, err := s.PushOperations("a", []SyncOperation{deleteOp})
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Acknowledge("a", result[0].Cursor); err != nil {
		t.Fatal(err)
	}
	if err := s.Acknowledge("b", result[0].Cursor-1); err != nil {
		t.Fatal(err)
	}
	if n, err := s.CompactTombstones(); err != nil || n != 0 {
		t.Fatalf("early compact = %d, %v", n, err)
	}
	if err := s.Acknowledge("b", result[0].Cursor); err != nil {
		t.Fatal(err)
	}
	if n, err := s.CompactTombstones(); err != nil || n != 1 {
		t.Fatalf("compact = %d, %v", n, err)
	}
}

func TestVersionEndpoint(t *testing.T) {
	server, err := NewServer(filepath.Join(t.TempDir(), "server.sqlite"))
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	mux := http.NewServeMux()
	server.Register(mux)
	rr := httptest.NewRecorder()
	mux.ServeHTTP(rr, httptest.NewRequest(http.MethodGet, "/api/sync/version", nil))
	if rr.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d", rr.Code, http.StatusOK)
	}
	var response map[string]string
	if err := json.NewDecoder(rr.Body).Decode(&response); err != nil {
		t.Fatal(err)
	}
	if response["version"] != version.Version {
		t.Errorf("version = %q, want %q", response["version"], version.Version)
	}
}

func TestSignedSyncFlow(t *testing.T) {
	server, err := NewServer(filepath.Join(t.TempDir(), "server.sqlite"))
	if err != nil {
		t.Fatalf("NewServer: %v", err)
	}
	defer server.Close()

	mux := http.NewServeMux()
	server.Register(mux)
	ts := httptest.NewServer(mux)
	defer ts.Close()

	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatalf("GenerateKey: %v", err)
	}

	enrollBody, _ := json.Marshal(map[string]string{
		"client_id":  "client-a",
		"label":      "client a",
		"public_key": base64.StdEncoding.EncodeToString(pub),
	})
	resp, err := http.Post(ts.URL+"/api/sync/enroll", "application/json", bytes.NewReader(enrollBody))
	if err != nil {
		t.Fatalf("enroll: %v", err)
	}
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("enroll status = %s", resp.Status)
	}
	_ = resp.Body.Close()

	changeBody, _ := json.Marshal(map[string]interface{}{
		"changes": []Change{{
			Op:         "upsert_item",
			TargetKind: "item",
			ID:         "abc1234",
			Path:       "abc/abc1234-note.md",
			Frontmatter: map[string]interface{}{
				"id":      "abc1234",
				"type":    "note",
				"title":   "Test",
				"created": "2026-01-01",
				"tags":    []interface{}{"sync"},
			},
			Body: "Body\n",
		}},
	})
	req, _ := http.NewRequest(http.MethodPost, ts.URL+"/api/sync/changes", bytes.NewReader(changeBody))
	req.Header.Set("Content-Type", "application/json")
	SignRequest(req, changeBody, "client-a", priv)
	resp, err = http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("pending change request: %v", err)
	}
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("pending client status = %s, want 401", resp.Status)
	}
	_ = resp.Body.Close()

	if err := server.ApproveClient("client-a"); err != nil {
		t.Fatalf("approve: %v", err)
	}

	req, _ = http.NewRequest(http.MethodPost, ts.URL+"/api/sync/changes", bytes.NewReader(changeBody))
	req.Header.Set("Content-Type", "application/json")
	SignRequest(req, changeBody, "client-a", priv)
	resp, err = http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("approved change request: %v", err)
	}
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("approved change status = %s", resp.Status)
	}
	_ = resp.Body.Close()

	req, _ = http.NewRequest(http.MethodGet, ts.URL+"/api/sync/snapshot", nil)
	SignRequest(req, nil, "client-a", priv)
	resp, err = http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("snapshot: %v", err)
	}
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("snapshot status = %s", resp.Status)
	}
	defer resp.Body.Close()

	var snapshot struct {
		Items []Change `json:"items"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&snapshot); err != nil {
		t.Fatalf("decode snapshot: %v", err)
	}
	if len(snapshot.Items) != 1 || snapshot.Items[0].ID != "abc1234" {
		t.Fatalf("snapshot items = %+v", snapshot.Items)
	}
}

func TestLoadConfigFromDBParsesConfigWithoutTempDir(t *testing.T) {
	server, err := NewServer(filepath.Join(t.TempDir(), "server.sqlite"))
	if err != nil {
		t.Fatalf("NewServer: %v", err)
	}
	defer server.Close()

	t.Setenv("TMPDIR", filepath.Join(t.TempDir(), "missing"))

	_, err = server.db.Exec(`
		INSERT INTO configs(path, content, revision, updated_at, deleted_at)
		VALUES (?, ?, ?, ?, '')
`, "exo.fnl", `{:views {:notes {:name "Notes" :key "n" :when (fn [note] (= note.type "note"))}} :actions {}}
`, 1, "2026-07-15T08:38:14Z")
	if err != nil {
		t.Fatalf("insert config: %v", err)
	}

	cfg, err := LoadConfigFromDB(server.db)
	if err != nil {
		t.Fatalf("LoadConfigFromDB: %v", err)
	}
	if _, err := os.Stat(os.Getenv("TMPDIR")); !os.IsNotExist(err) {
		t.Fatalf("TMPDIR exists or stat failed with unexpected error: %v", err)
	}
	if cfg.Views["notes"].Name != "Notes" {
		t.Fatalf("notes view = %+v", cfg.Views["notes"])
	}
	if cfg.Views["notes"].TitleField != "title" {
		t.Fatalf("title default = %q, want title", cfg.Views["notes"].TitleField)
	}
	if _, ok := cfg.Views["all"]; !ok {
		t.Fatal("built-in all view missing")
	}
}
