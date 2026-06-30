package handlers

import (
	"encoding/json"
	"net/http"
)

// APIResponse is the JSON response structure for the /api/get/<id> endpoint.
type APIResponse struct {
	Frontmatter map[string]interface{} `json:"frontmatter"`
	Body        string                 `json:"body"`
}

// GetItemByID returns an item (note, book, webhook, etc.) as JSON
// with frontmatter and body split.
// GET /api/get/{id}
func (h *Handlers) GetItemByID(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if id == "" {
		http.Error(w, `{"error": "missing id"}`, http.StatusBadRequest)
		return
	}

	item, err := h.Cache.GetByID(id)
	if err != nil {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusNotFound)
		w.Write([]byte(`{"error": "item not found"}`))
		return
	}

	targetItem := &APIResponse{
		Frontmatter: item.Frontmatter,
		Body:        item.Body,
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	if err := json.NewEncoder(w).Encode(targetItem); err != nil {
		http.Error(w, `{"error": "encoding error"}`, http.StatusInternalServerError)
	}
}
