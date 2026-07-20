package tui

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

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/syncsvc"
	"github.com/gnur/exokephalos/internal/version"
)

func TestCheckServerVersion(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]string{"version": version.Version})
	}))
	defer server.Close()
	if err := checkServerVersion(server.URL); err != nil {
		t.Fatalf("matching versions: %v", err)
	}
}

func TestCheckServerVersionMismatch(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]string{"version": "other"})
	}))
	defer server.Close()
	if err := checkServerVersion(server.URL); err == nil {
		t.Fatal("expected version mismatch")
	}
}

func TestSync_Deletions(t *testing.T) {
	// 1. Setup sync server
	server, err := syncsvc.NewServer(filepath.Join(t.TempDir(), "server.sqlite"))
	if err != nil {
		t.Fatalf("NewServer: %v", err)
	}
	defer server.Close()

	mux := http.NewServeMux()
	server.Register(mux)
	ts := httptest.NewServer(mux)
	defer ts.Close()

	// 2. Setup Client A
	dirA := t.TempDir()
	pubA, privA, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	cA, err := cache.New(dirA)
	if err != nil {
		t.Fatal(err)
	}
	defer cA.Close()

	// 3. Setup Client B
	dirB := t.TempDir()
	pubB, privB, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	cB, err := cache.New(dirB)
	if err != nil {
		t.Fatal(err)
	}
	defer cB.Close()

	if err := cA.SetSyncStarted(true); err != nil {
		t.Fatal(err)
	}
	if err := cB.SetSyncStarted(true); err != nil {
		t.Fatal(err)
	}

	// 4. Enroll & approve clients
	enrollClient(t, ts.URL, "client-a", pubA, server)
	enrollClient(t, ts.URL, "client-b", pubB, server)

	// 5. Create a file on Client A
	notePathA := filepath.Join(dirA, "note1.md")
	fm := map[string]interface{}{"id": "note1", "type": "note", "title": "Note 1"}
	if err := markdown.WriteFrontmatter(notePathA, fm, "Body of note 1\n"); err != nil {
		t.Fatal(err)
	}
	if err := cA.NotifyWrite(notePathA); err != nil {
		t.Fatal(err)
	}

	// 6. Push Client A changes
	if err := pushOutbox(ts.URL, "client-a", privA, cA); err != nil {
		t.Fatalf("push client-a: %v", err)
	}

	// 7. Pull on Client B
	if _, err := pullSnapshot(dirB, ts.URL, "client-b", privB, cB); err != nil {
		t.Fatalf("pull client-b: %v", err)
	}

	// Verify note exists on B
	notePathB := filepath.Join(dirB, "note1.md")
	if _, err := os.Stat(notePathB); err != nil {
		t.Fatalf("expected note to exist on B: %v", err)
	}

	// 8. Delete note on Client A
	if err := os.Remove(notePathA); err != nil {
		t.Fatal(err)
	}
	if err := cA.NotifyDelete(notePathA); err != nil {
		t.Fatal(err)
	}

	// 9. Push Client A deletion
	if err := pushOutbox(ts.URL, "client-a", privA, cA); err != nil {
		t.Fatalf("push client-a deletion: %v", err)
	}

	// 10. Pull on Client B
	if _, err := pullSnapshot(dirB, ts.URL, "client-b", privB, cB); err != nil {
		t.Fatalf("pull client-b after deletion: %v", err)
	}

	// Verify note was deleted on B
	if _, err := os.Stat(notePathB); !os.IsNotExist(err) {
		t.Fatalf("expected note to be deleted on B, but stat got: %v", err)
	}
}

func TestSync_Assets(t *testing.T) {
	serverDir := t.TempDir()
	server, err := syncsvc.NewServer(filepath.Join(serverDir, "server.sqlite"))
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	server.SetBaseDir(serverDir)
	mux := http.NewServeMux()
	server.Register(mux)
	ts := httptest.NewServer(mux)
	defer ts.Close()

	dirA, dirB := t.TempDir(), t.TempDir()
	pubA, privA, _ := ed25519.GenerateKey(rand.Reader)
	pubB, privB, _ := ed25519.GenerateKey(rand.Reader)
	cA, err := cache.New(dirA)
	if err != nil {
		t.Fatal(err)
	}
	defer cA.Close()
	cB, err := cache.New(dirB)
	if err != nil {
		t.Fatal(err)
	}
	defer cB.Close()
	if err := cA.SetSyncStarted(true); err != nil {
		t.Fatal(err)
	}
	if err := cB.SetSyncStarted(true); err != nil {
		t.Fatal(err)
	}
	enrollClient(t, ts.URL, "asset-a", pubA, server)
	enrollClient(t, ts.URL, "asset-b", pubB, server)

	// A minimal valid PNG, sufficient for content-type validation.
	png := []byte{0x89, 'P', 'N', 'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0}
	if err := os.MkdirAll(filepath.Join(dirA, "assets"), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dirA, "assets", "photo.png"), png, 0644); err != nil {
		t.Fatal(err)
	}
	if err := cA.Sync(); err != nil {
		t.Fatal(err)
	}
	if err := pushOutbox(ts.URL, "asset-a", privA, cA); err != nil {
		t.Fatal(err)
	}
	if _, err := pullSnapshot(dirB, ts.URL, "asset-b", privB, cB); err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(filepath.Join(dirB, "assets", "photo.png"))
	if err != nil {
		t.Fatalf("asset not downloaded: %v", err)
	}
	if !bytes.Equal(got, png) {
		t.Fatalf("asset content = %x, want %x", got, png)
	}

	if err := os.Remove(filepath.Join(dirA, "assets", "photo.png")); err != nil {
		t.Fatal(err)
	}
	if err := cA.Sync(); err != nil {
		t.Fatal(err)
	}
	if err := pushOutbox(ts.URL, "asset-a", privA, cA); err != nil {
		t.Fatal(err)
	}
	if _, err := pullSnapshot(dirB, ts.URL, "asset-b", privB, cB); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dirB, "assets", "photo.png")); !os.IsNotExist(err) {
		t.Fatalf("asset was not deleted: %v", err)
	}
}

func enrollClient(t *testing.T, url, clientID string, pub ed25519.PublicKey, server *syncsvc.Server) {
	t.Helper()
	enrollBody, _ := json.Marshal(map[string]string{
		"client_id":  clientID,
		"label":      clientID,
		"public_key": base64.StdEncoding.EncodeToString(pub),
	})
	resp, err := http.Post(url+"/api/sync/enroll", "application/json", bytes.NewReader(enrollBody))
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("enroll status = %s", resp.Status)
	}
	if err := server.ApproveClient(clientID); err != nil {
		t.Fatal(err)
	}
}
