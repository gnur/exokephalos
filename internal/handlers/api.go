package handlers

import (
	"encoding/json"
	"io"
	"net/http"
	"sort"
	"strings"

	"github.com/gnur/exokephalos/internal/auth"
	"github.com/gnur/exokephalos/internal/filter"
	"github.com/gnur/exokephalos/internal/urlimport"
)

// APIItem is the JSON representation of an item.
type APIItem struct {
	Frontmatter map[string]interface{} `json:"frontmatter"`
	Body        string                 `json:"body"`
}

type UpdateItemRequest struct {
	Frontmatter *map[string]interface{} `json:"frontmatter"`
	Body        *string                 `json:"body"`
}

type CreateItemRequest struct {
	URL string `json:"url"`
}

type QueryIDsResponse struct {
	IDs []string `json:"ids"`
}

// GetItemByID returns an item (note, book, webhook, etc.) as JSON
// with frontmatter and body split.
// GET /api/items/{id}
func (h *Handlers) GetItemByID(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if id == "" {
		writeAPIError(w, "missing id", http.StatusBadRequest)
		return
	}

	item, err := h.Store.GetByID(id)
	if err != nil {
		writeAPIError(w, "item not found", http.StatusNotFound)
		return
	}
	matches, err := apiKeyFilterMatches(r, item.Frontmatter)
	if err != nil {
		writeAPIError(w, "API key filter evaluation failed", http.StatusInternalServerError)
		return
	}
	if !matches {
		writeAPIError(w, "item not found", http.StatusNotFound)
		return
	}

	targetItem := &APIItem{
		Frontmatter: item.Frontmatter,
		Body:        item.Body,
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	if err := json.NewEncoder(w).Encode(targetItem); err != nil {
		http.Error(w, `{"error": "encoding error"}`, http.StatusInternalServerError)
	}
}

// CreateItem creates a new item from a supported source.
// POST /api/items
func (h *Handlers) CreateItem(w http.ResponseWriter, r *http.Request) {
	var req CreateItemRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeAPIError(w, "invalid JSON body", http.StatusBadRequest)
		return
	}
	if strings.TrimSpace(req.URL) == "" {
		writeAPIError(w, "missing url", http.StatusBadRequest)
		return
	}

	result, err := urlimport.Build(r.Context(), h.BaseDir, req.URL)
	if err != nil {
		writeAPIError(w, err.Error(), http.StatusBadRequest)
		return
	}
	if err := h.Store.CreateItem(result.Path, result.Frontmatter, result.Body); err != nil {
		writeAPIError(w, err.Error(), http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	if err := json.NewEncoder(w).Encode(struct {
		ID          string                 `json:"id"`
		Frontmatter map[string]interface{} `json:"frontmatter"`
		Body        string                 `json:"body"`
	}{
		ID:          result.ID,
		Frontmatter: result.Frontmatter,
		Body:        result.Body,
	}); err != nil {
		http.Error(w, `{"error": "encoding error"}`, http.StatusInternalServerError)
	}
}

// UpdateItemByID updates an item's frontmatter and/or body.
// PATCH /api/items/{id}
func (h *Handlers) UpdateItemByID(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if id == "" {
		writeAPIError(w, "missing id", http.StatusBadRequest)
		return
	}

	var req UpdateItemRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeAPIError(w, "invalid JSON body", http.StatusBadRequest)
		return
	}
	if req.Frontmatter == nil && req.Body == nil {
		writeAPIError(w, "missing frontmatter or body", http.StatusBadRequest)
		return
	}

	item, err := h.Store.GetByID(id)
	if err != nil {
		writeAPIError(w, "item not found", http.StatusNotFound)
		return
	}
	matches, err := apiKeyFilterMatches(r, item.Frontmatter)
	if err != nil {
		writeAPIError(w, "API key filter evaluation failed", http.StatusInternalServerError)
		return
	}
	if !matches {
		writeAPIError(w, "item not found", http.StatusNotFound)
		return
	}

	fm := item.Frontmatter
	if req.Frontmatter != nil {
		fm = *req.Frontmatter
	}
	body := item.Body
	if req.Body != nil {
		body = *req.Body
	}
	matches, err = apiKeyFilterMatches(r, fm)
	if err != nil {
		writeAPIError(w, "API key filter evaluation failed", http.StatusInternalServerError)
		return
	}
	if !matches {
		writeAPIError(w, "updated item does not match API key filter", http.StatusForbidden)
		return
	}

	if err := h.Store.UpdateItem(item.Path, fm, body); err != nil {
		writeAPIError(w, "updating item", http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	if err := json.NewEncoder(w).Encode(APIItem{Frontmatter: fm, Body: body}); err != nil {
		http.Error(w, `{"error": "encoding error"}`, http.StatusInternalServerError)
	}
}

func apiKeyFilterMatches(r *http.Request, frontmatter map[string]interface{}) (bool, error) {
	key, ok := auth.APIKeyFromContext(r.Context())
	if !ok {
		return true, nil
	}
	prog, err := filter.Compile(key.Filter)
	if err != nil {
		return false, err
	}
	return prog.Eval(frontmatter)
}

// QueryIDsByCEL returns IDs for all cached items matching a CEL expression.
// POST /api/query/ids
func (h *Handlers) QueryIDsByCEL(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(r.Body)
	if err != nil {
		writeAPIError(w, "reading request body", http.StatusBadRequest)
		return
	}

	expr := strings.TrimSpace(string(body))
	if expr == "" {
		writeAPIError(w, "missing expr", http.StatusBadRequest)
		return
	}

	prog, err := filter.Compile(expr)
	if err != nil {
		writeAPIError(w, "invalid CEL expression", http.StatusBadRequest)
		return
	}
	var apiKeyProg *filter.Program
	if key, ok := auth.APIKeyFromContext(r.Context()); ok {
		apiKeyProg, err = filter.Compile(key.Filter)
		if err != nil {
			writeAPIError(w, "invalid API key filter", http.StatusInternalServerError)
			return
		}
	}

	items, err := h.Store.All()
	if err != nil {
		writeAPIError(w, "cache read error", http.StatusInternalServerError)
		return
	}

	ids := make([]string, 0)
	for _, item := range items {
		ok, err := prog.Eval(item.Frontmatter)
		if err != nil {
			writeAPIError(w, "CEL evaluation error", http.StatusBadRequest)
			return
		}
		if !ok {
			continue
		}
		if apiKeyProg != nil {
			ok, err = apiKeyProg.Eval(item.Frontmatter)
			if err != nil {
				writeAPIError(w, "API key filter evaluation failed", http.StatusInternalServerError)
				return
			}
			if !ok {
				continue
			}
		}
		id, ok := item.Frontmatter["id"].(string)
		if !ok || id == "" {
			continue
		}
		ids = append(ids, id)
	}
	sort.Strings(ids)

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	if err := json.NewEncoder(w).Encode(QueryIDsResponse{IDs: ids}); err != nil {
		http.Error(w, `{"error": "encoding error"}`, http.StatusInternalServerError)
	}
}

func writeAPIError(w http.ResponseWriter, message string, status int) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": message})
}
