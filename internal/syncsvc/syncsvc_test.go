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

	"github.com/gnur/exokephalos/internal/version"
)

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
`, "exo.fnl", `{:views {:notes {:name "Notes" :key "n" :when (fn [note] (= note.type "note")) :template "---\ntype: note\n---\n"}} :actions {}}
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
