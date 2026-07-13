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
)

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
