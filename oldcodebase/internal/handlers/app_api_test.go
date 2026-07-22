package handlers

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"testing"

	"github.com/gnur/exokephalos/internal/syncsvc"
)

func TestAppSyncClientsReturnsEmptyArray(t *testing.T) {
	server, err := syncsvc.NewServer(filepath.Join(t.TempDir(), "server.sqlite"))
	if err != nil {
		t.Fatalf("NewServer: %v", err)
	}
	defer server.Close()

	h := &Handlers{SyncServer: server}
	rr := httptest.NewRecorder()
	h.AppSyncClients(rr, httptest.NewRequest(http.MethodGet, "/api/app/sync-clients", nil))

	if rr.Code != http.StatusOK {
		t.Fatalf("status = %d, body = %s", rr.Code, rr.Body.String())
	}

	var body struct {
		Clients []syncsvc.Client `json:"clients"`
	}
	if err := json.NewDecoder(rr.Body).Decode(&body); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	if body.Clients == nil {
		t.Fatal("clients decoded as nil, want empty array")
	}
	if len(body.Clients) != 0 {
		t.Fatalf("clients length = %d, want 0", len(body.Clients))
	}
}
