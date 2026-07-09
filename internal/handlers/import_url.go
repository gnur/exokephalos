package handlers

import (
	"net/http"
	"strings"
	"time"

	"github.com/gnur/exokephalos/internal/urlimport"
)

// ImportURL handles GET/POST /import-url — create a note from a URL.
func (h *Handlers) ImportURL(w http.ResponseWriter, r *http.Request) {
	data := newData(r)
	data["_parseTime"] = time.Duration(0)

	if r.Method == http.MethodGet {
		h.render(w, r, "views/import_url.html", data)
		return
	}

	if err := r.ParseForm(); err != nil {
		data["Error"] = "Invalid form"
		h.render(w, r, "views/import_url.html", data)
		return
	}
	rawURL := strings.TrimSpace(r.FormValue("url"))
	if rawURL == "" {
		data["Error"] = "URL is required"
		h.render(w, r, "views/import_url.html", data)
		return
	}

	result, err := urlimport.Import(r.Context(), h.Repo, h.BaseDir, rawURL)
	if err != nil {
		data["Error"] = err.Error()
		data["URL"] = rawURL
		h.render(w, r, "views/import_url.html", data)
		return
	}

	http.Redirect(w, r, "/views/all/"+result.ID, http.StatusSeeOther)
}
