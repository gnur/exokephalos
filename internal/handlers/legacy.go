package handlers

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"path/filepath"
	"strings"
	"time"
)

// WebhookReceive handles POST /webhook/{source} — ingests incoming webhooks.
func (h *Handlers) WebhookReceive(w http.ResponseWriter, r *http.Request) {
	source := r.PathValue("source")
	typ := r.URL.Query().Get("typ")
	if typ == "" {
		typ = "webhook"
	}

	bodyBytes, err := io.ReadAll(http.MaxBytesReader(w, r.Body, 2*1024*1024))
	if err != nil {
		http.Error(w, "Request body too large", http.StatusRequestEntityTooLarge)
		return
	}
	bodyText := string(bodyBytes)
	bodyLang := ""

	var js interface{}
	if json.Unmarshal(bodyBytes, &js) == nil {
		pretty, _ := json.MarshalIndent(js, "", "  ")
		bodyText = string(pretty)
		bodyLang = "json"
	}

	if bodyText == "" {
		bodyText = "<empty>"
	}

	headers := map[string]string{}
	for k, v := range r.Header {
		if len(v) > 0 {
			headers[k] = v[0]
		}
	}

	now := time.Now()
	subdir := filepath.Join(h.BaseDir, "webhooks", fmt.Sprintf("%d", now.Year()), fmt.Sprintf("%02d", now.Month()), fmt.Sprintf("%02d", now.Day()))
	filename := fmt.Sprintf("%s-%02d-%02d-%02d.md", source, now.Hour(), now.Minute(), now.Second())
	path := filepath.Join(subdir, filename)

	fm := map[string]interface{}{
		"timestamp": now.Format("2006-01-02T15:04:05") + "Z",
		"source":    source,
		"type":      typ,
	}

	var body strings.Builder
	fmt.Fprintf(&body, "```%s\n%s\n```\n\n", bodyLang, bodyText)
	body.WriteString("```yaml\n")
	for k, v := range headers {
		fmt.Fprintf(&body, "%s: %s\n", k, v)
	}
	body.WriteString("```\n")

	if err := h.Repo.CreateItem(path, fm, body.String()); err != nil {
		http.Error(w, err.Error(), 500)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	_, _ = w.Write([]byte(`{"status":"success"}`))
}
