package handlers

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gnur/exokephalos/internal/auth"
	"github.com/gnur/exokephalos/internal/scanner"
)

type testItemStore struct {
	items map[string]scanner.Item
}

func (s testItemStore) All() ([]scanner.Item, error) {
	items := make([]scanner.Item, 0, len(s.items))
	for _, item := range s.items {
		items = append(items, item)
	}
	return items, nil
}

func (s testItemStore) GetByID(id string) (*scanner.Item, error) {
	item, ok := s.items[id]
	if !ok {
		return nil, fmt.Errorf("not found")
	}
	return &item, nil
}

func (s testItemStore) ReadRaw(string) (string, error) { return "", nil }
func (s testItemStore) WriteRaw(string, string) error  { return nil }
func (s testItemStore) CreateItem(string, map[string]interface{}, string) error {
	return nil
}
func (s testItemStore) UpdateItem(string, map[string]interface{}, string) error {
	return nil
}
func (s testItemStore) DeleteItem(string) error { return nil }

func TestGetItemByIDWithAPIKeyFilters(t *testing.T) {
	authMgr, err := auth.New(filepath.Join(t.TempDir(), "auth.sqlite"))
	if err != nil {
		t.Fatalf("auth.New: %v", err)
	}
	defer authMgr.Close()

	raw, _, err := authMgr.CreateAPIKey("notes app", `type == "note"`, time.Now().Add(24*time.Hour))
	if err != nil {
		t.Fatalf("CreateAPIKey: %v", err)
	}
	h := &Handlers{Store: testItemStore{items: map[string]scanner.Item{
		"note1": {
			Frontmatter: map[string]interface{}{"id": "note1", "type": "note", "title": "Note"},
			Body:        "note body",
		},
		"secret1": {
			Frontmatter: map[string]interface{}{"id": "secret1", "type": "secret", "title": "Secret", "tags": []interface{}{"acceptance"}},
			Body:        "secret body",
		},
	}}}
	server := apiKeyTestServer(authMgr, h)

	req := httptest.NewRequest(http.MethodGet, "/api/items/note1", nil)
	req.Header.Set("Authorization", "Bearer "+raw)
	rr := httptest.NewRecorder()
	server.ServeHTTP(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("matching item status = %d body=%s", rr.Code, rr.Body.String())
	}
	var item APIItem
	if err := json.NewDecoder(rr.Body).Decode(&item); err != nil {
		t.Fatalf("decode item: %v", err)
	}
	if item.Body != "note body" {
		t.Fatalf("body = %q", item.Body)
	}

	req = httptest.NewRequest(http.MethodGet, "/api/items/secret1", nil)
	req.Header.Set("Authorization", "Bearer "+raw)
	rr = httptest.NewRecorder()
	server.ServeHTTP(rr, req)
	if rr.Code != http.StatusNotFound {
		t.Fatalf("non-matching item status = %d body=%s, want 404", rr.Code, rr.Body.String())
	}
}

func TestGetItemByIDWithXAPIKey(t *testing.T) {
	authMgr, err := auth.New(filepath.Join(t.TempDir(), "auth.sqlite"))
	if err != nil {
		t.Fatalf("auth.New: %v", err)
	}
	defer authMgr.Close()

	raw, _, err := authMgr.CreateAPIKey("secret app", `type == "secret" && "acceptance" in tags`, time.Now().Add(24*time.Hour))
	if err != nil {
		t.Fatalf("CreateAPIKey: %v", err)
	}
	h := &Handlers{Store: testItemStore{items: map[string]scanner.Item{
		"secret1": {
			Frontmatter: map[string]interface{}{"id": "secret1", "type": "secret", "title": "Secret", "tags": []interface{}{"acceptance"}},
			Body:        "secret body",
		},
	}}}
	server := apiKeyTestServer(authMgr, h)

	req := httptest.NewRequest(http.MethodGet, "/api/items/secret1", nil)
	req.Header.Set("X-API-Key", raw)
	rr := httptest.NewRecorder()
	server.ServeHTTP(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("status = %d body=%s, want 200", rr.Code, rr.Body.String())
	}
}

func TestUpdateItemByIDWithAPIKeyFilters(t *testing.T) {
	authMgr, err := auth.New(filepath.Join(t.TempDir(), "auth.sqlite"))
	if err != nil {
		t.Fatalf("auth.New: %v", err)
	}
	defer authMgr.Close()

	raw, _, err := authMgr.CreateAPIKey("notes app", `type == "note"`, time.Now().Add(24*time.Hour))
	if err != nil {
		t.Fatalf("CreateAPIKey: %v", err)
	}
	h := &Handlers{Store: testItemStore{items: map[string]scanner.Item{
		"note1": {
			Frontmatter: map[string]interface{}{"id": "note1", "type": "note", "title": "Note"},
			Body:        "note body",
		},
		"secret1": {
			Frontmatter: map[string]interface{}{"id": "secret1", "type": "secret", "title": "Secret"},
			Body:        "secret body",
		},
	}}}
	server := apiKeyTestServer(authMgr, h)

	request := func(id, body string) *httptest.ResponseRecorder {
		req := httptest.NewRequest(http.MethodPatch, "/api/items/"+id, strings.NewReader(body))
		req.Header.Set("Authorization", "Bearer "+raw)
		req.Header.Set("Content-Type", "application/json")
		rr := httptest.NewRecorder()
		server.ServeHTTP(rr, req)
		return rr
	}

	if rr := request("note1", `{"body":"updated note body"}`); rr.Code != http.StatusOK {
		t.Fatalf("matching item status = %d body=%s, want 200", rr.Code, rr.Body.String())
	}
	if rr := request("secret1", `{"body":"updated secret body"}`); rr.Code != http.StatusNotFound {
		t.Fatalf("non-matching item status = %d body=%s, want 404", rr.Code, rr.Body.String())
	}
	if rr := request("note1", `{"frontmatter":{"id":"note1","type":"secret","title":"Secret"}}`); rr.Code != http.StatusForbidden {
		t.Fatalf("out-of-scope update status = %d body=%s, want 403", rr.Code, rr.Body.String())
	}
}

func TestQueryIDsByCELWithAPIKeyFilter(t *testing.T) {
	authMgr, err := auth.New(filepath.Join(t.TempDir(), "auth.sqlite"))
	if err != nil {
		t.Fatalf("auth.New: %v", err)
	}
	defer authMgr.Close()

	raw, _, err := authMgr.CreateAPIKey("books app", `type == "book"`, time.Now().Add(24*time.Hour))
	if err != nil {
		t.Fatalf("CreateAPIKey: %v", err)
	}
	h := &Handlers{Store: testItemStore{items: map[string]scanner.Item{
		"book1": {Frontmatter: map[string]interface{}{"id": "book1", "type": "book", "tags": []interface{}{"to-read"}}},
		"book2": {Frontmatter: map[string]interface{}{"id": "book2", "type": "book", "tags": []interface{}{"read"}}},
		"note1": {Frontmatter: map[string]interface{}{"id": "note1", "type": "note", "tags": []interface{}{"to-read"}}},
	}}}
	server := apiKeyTestServer(authMgr, h)

	req := httptest.NewRequest(http.MethodPost, "/api/query/ids", strings.NewReader(`"to-read" in tags`))
	req.Header.Set("Authorization", "Bearer "+raw)
	rr := httptest.NewRecorder()
	server.ServeHTTP(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("status = %d body=%s, want 200", rr.Code, rr.Body.String())
	}
	var result QueryIDsResponse
	if err := json.NewDecoder(rr.Body).Decode(&result); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	if len(result.IDs) != 1 || result.IDs[0] != "book1" {
		t.Fatalf("ids = %v, want [book1]", result.IDs)
	}

	req = httptest.NewRequest(http.MethodPost, "/api/query/ids", strings.NewReader(`true`))
	rr = httptest.NewRecorder()
	server.ServeHTTP(rr, req)
	if rr.Code != http.StatusUnauthorized {
		t.Fatalf("missing key status = %d body=%s, want 401", rr.Code, rr.Body.String())
	}
}

func TestAPIKeyDoesNotAuthenticateOtherRoutes(t *testing.T) {
	authMgr, err := auth.New(filepath.Join(t.TempDir(), "auth.sqlite"))
	if err != nil {
		t.Fatalf("auth.New: %v", err)
	}
	defer authMgr.Close()

	raw, _, err := authMgr.CreateAPIKey("notes app", `true`, time.Now().Add(24*time.Hour))
	if err != nil {
		t.Fatalf("CreateAPIKey: %v", err)
	}
	mux := http.NewServeMux()
	mux.HandleFunc("GET /api/app/bootstrap", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	})
	server := authMgr.Middleware(mux)

	req := httptest.NewRequest(http.MethodGet, "/api/app/bootstrap", nil)
	req.Header.Set("Authorization", "Bearer "+raw)
	rr := httptest.NewRecorder()
	server.ServeHTTP(rr, req)
	if rr.Code != http.StatusUnauthorized {
		t.Fatalf("status = %d, want 401", rr.Code)
	}
}

func apiKeyTestServer(authMgr *auth.Manager, h *Handlers) http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /api/items/{id}", h.GetItemByID)
	mux.HandleFunc("PATCH /api/items/{id}", h.UpdateItemByID)
	mux.HandleFunc("POST /api/query/ids", h.QueryIDsByCEL)
	return authMgr.Middleware(mux)
}
