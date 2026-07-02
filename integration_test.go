package main

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/handlers"
	"github.com/gnur/exokephalos/internal/repo"
)

func setupTestServer(t *testing.T) (*httptest.Server, string) {
	t.Helper()

	// Copy example-repo to a temp dir so tests don't pollute real data
	tmpDir, err := os.MkdirTemp("", "exo-test-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}

	cmd := exec.Command("cp", "-a", "./example-repo/.", tmpDir)
	if err := cmd.Run(); err != nil {
		os.RemoveAll(tmpDir)
		t.Fatalf("Failed to copy example-repo: %v", err)
	}

	cfg, err := config.Load(tmpDir)
	if err != nil {
		os.RemoveAll(tmpDir)
		t.Fatalf("Failed to load config: %v", err)
	}

	c, err := cache.New(tmpDir)
	if err != nil {
		os.RemoveAll(tmpDir)
		t.Fatalf("Failed to create cache: %v", err)
	}
	t.Cleanup(func() { c.Close() })

	r := repo.New(tmpDir, c)
	h, err := handlers.New(cfg, tmpDir, r, c, os.DirFS("."))
	if err != nil {
		os.RemoveAll(tmpDir)
		t.Fatalf("Failed to create handlers: %v", err)
	}

	mux := http.NewServeMux()
	mux.Handle("GET /static/", http.StripPrefix("/static/", http.FileServer(http.Dir("static"))))

	// API endpoints
	mux.HandleFunc("GET /api/get/{id}", h.GetItemByID)

	// Generic view routes
	mux.HandleFunc("GET /views/{viewId}/stats", h.ViewStats)
	mux.HandleFunc("GET /views/{viewId}/new", h.ViewNew)
	mux.HandleFunc("POST /views/{viewId}/new", h.ViewNew)
	mux.HandleFunc("GET /views/{viewId}/edit/{itemId}", h.ViewEdit)
	mux.HandleFunc("POST /views/{viewId}/edit/{itemId}", h.ViewEdit)
	mux.HandleFunc("POST /views/{viewId}/delete/{itemId}", h.ViewDelete)
	mux.HandleFunc("GET /views/{viewId}/{itemId}", h.ViewDetail)
	mux.HandleFunc("GET /views/{viewId}", h.ViewList)

	// Hardcoded API endpoints
	mux.HandleFunc("POST /webhook/{source}", h.WebhookReceive)

	// Root redirect
	defaultView := cfg.DefaultView
	if defaultView == "" {
		views := cfg.OrderedViews()
		if len(views) > 0 {
			defaultView = views[0].ID
		}
	}
	redirectTarget := "/views/" + defaultView
	mux.HandleFunc("GET /", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/" {
			http.Redirect(w, r, redirectTarget, http.StatusSeeOther)
			return
		}
		http.NotFound(w, r)
	})

	return httptest.NewServer(h.TimingMiddleware(h.CSRFMiddleware(mux))), tmpDir
}

func TestRootRedirects(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	client := &http.Client{CheckRedirect: func(req *http.Request, via []*http.Request) error {
		return http.ErrUseLastResponse
	}}
	resp, err := client.Get(srv.URL + "/")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != http.StatusSeeOther {
		t.Errorf("expected 303, got %d", resp.StatusCode)
	}
	if loc := resp.Header.Get("Location"); loc != "/views/notes" {
		t.Errorf("expected redirect to /views/notes, got %s", loc)
	}
}

func TestViewList_Notes(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Get(srv.URL + "/views/notes")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
	body, _ := io.ReadAll(resp.Body)
	s := string(body)

	assertContains(t, s, "exokephalos")
	assertContains(t, s, "Notes")
	assertContains(t, s, "Total:")
	assertContains(t, s, "Parsing:")
}

func TestViewList_Books(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Get(srv.URL + "/views/books")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
	body, _ := io.ReadAll(resp.Body)
	s := string(body)
	assertContains(t, s, "Books")
	assertContains(t, s, "Total:")
}

func TestViewList_WithTagFilter(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Get(srv.URL + "/views/notes?tags=recept")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
	body, _ := io.ReadAll(resp.Body)
	s := string(body)
	// Active tag should be highlighted
	assertContains(t, s, "recept")
}

func TestViewList_WithSubview(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Get(srv.URL + "/views/books?subview=Reading")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
	body, _ := io.ReadAll(resp.Body)
	assertContains(t, string(body), "Reading")
}

func TestViewDetail(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	// Get a note from the list
	resp, err := http.Get(srv.URL + "/views/notes")
	if err != nil {
		t.Fatal(err)
	}
	body, _ := io.ReadAll(resp.Body)
	s := string(body)

	// Find item links, skipping /new and /stats
	var itemID string
	searchFrom := 0
	for {
		idx := strings.Index(s[searchFrom:], `href="/views/notes/`)
		if idx == -1 {
			break
		}
		pos := searchFrom + idx + len(`href="/views/notes/`)
		end := strings.Index(s[pos:], `"`)
		candidate := s[pos : pos+end]
		if candidate != "new" && candidate != "stats" && !strings.HasPrefix(candidate, "edit/") && !strings.HasPrefix(candidate, "delete/") {
			itemID = candidate
			break
		}
		searchFrom = pos + end
	}
	if itemID == "" {
		t.Skip("No item links found in notes list")
	}

	resp, err = http.Get(srv.URL + "/views/notes/" + itemID)
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200 for /views/notes/%s, got %d", itemID, resp.StatusCode)
	}
	body, _ = io.ReadAll(resp.Body)
	assertContains(t, string(body), "Notes")
}

func TestViewDetail_NotFound(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Get(srv.URL + "/views/notes/zzzzz-nonexistent")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 404 {
		t.Errorf("expected 404, got %d", resp.StatusCode)
	}
}

func TestViewNew(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Get(srv.URL + "/views/notes/new")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
	body, _ := io.ReadAll(resp.Body)
	s := string(body)
	assertContains(t, s, "New Notes")
	assertContains(t, s, `method="POST"`)
}

func TestViewCreateAndDelete(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	client := &http.Client{CheckRedirect: func(req *http.Request, via []*http.Request) error {
		return http.ErrUseLastResponse
	}}

	// Create a note
	form := url.Values{}
	form.Set("Title", "Integration Test Note")

	resp, err := client.PostForm(srv.URL+"/views/notes/new", form)
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != http.StatusSeeOther {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 303 redirect, got %d. Body: %s", resp.StatusCode, string(body))
	}

	// Verify the note appears in the list
	resp, err = http.Get(srv.URL + "/views/notes")
	if err != nil {
		t.Fatal(err)
	}
	body, _ := io.ReadAll(resp.Body)
	s := string(body)
	assertContains(t, s, "Integration Test Note")

	// Find the item ID from the link
	idx := strings.Index(s, "Integration Test Note")
	if idx == -1 {
		t.Fatal("Created note not found in listing")
	}
	// Look backward for the href
	before := s[:idx]
	hrefIdx := strings.LastIndex(before, `href="/views/notes/`)
	if hrefIdx == -1 {
		t.Fatal("Could not find href for created note")
	}
	start := hrefIdx + len(`href="/views/notes/`)
	end := strings.Index(s[start:], `"`)
	itemID := s[start : start+end]

	// Delete the note
	resp, err = client.PostForm(srv.URL+"/views/notes/delete/"+itemID, url.Values{})
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != http.StatusSeeOther {
		t.Errorf("expected 303 after delete, got %d", resp.StatusCode)
	}

	// Verify it's gone
	resp, err = http.Get(srv.URL + "/views/notes/" + itemID)
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 404 {
		t.Errorf("expected 404 after delete, got %d", resp.StatusCode)
	}
}

func TestBooksStats(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Get(srv.URL + "/views/books/stats")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
	body, _ := io.ReadAll(resp.Body)
	s := string(body)
	assertContains(t, s, "Book Stats")
	assertContains(t, s, "Books Read")
	assertContains(t, s, "Currently Reading")
	assertContains(t, s, "To Read")
	assertContains(t, s, "Books per Year")
	assertContains(t, s, "Total:")
}

func TestWebhookReceive(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	payload := `{"event":"test","data":"hello"}`
	resp, err := http.Post(srv.URL+"/webhook/test-source", "application/json", strings.NewReader(payload))
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
	body, _ := io.ReadAll(resp.Body)
	var result map[string]string
	if err := json.Unmarshal(body, &result); err != nil {
		t.Fatalf("invalid JSON response: %v", err)
	}
	if result["status"] != "success" {
		t.Errorf("expected status success, got %s", result["status"])
	}
}

func TestWebhookReceiveEmptyBody(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Post(srv.URL+"/webhook/empty-test", "text/plain", strings.NewReader(""))
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
}

func TestWebhookReceiveWithType(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Post(srv.URL+"/webhook/typed?typ=alert", "text/plain", strings.NewReader("alert body"))
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
}

func TestViewList_NonexistentView(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Get(srv.URL + "/views/nonexistent")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 404 {
		t.Errorf("expected 404, got %d", resp.StatusCode)
	}
}

func TestViewStats_NoStatsTemplate(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Get(srv.URL + "/views/notes/stats")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 404 {
		t.Errorf("expected 404 for view without stats_template, got %d", resp.StatusCode)
	}
}

func Test404(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Get(srv.URL + "/nonexistent")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 404 {
		t.Errorf("expected 404, got %d", resp.StatusCode)
	}
}

func TestAPIGetItem(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	// Get a note from the list
	resp, err := http.Get(srv.URL + "/views/notes")
	if err != nil {
		t.Fatal(err)
	}
	body, _ := io.ReadAll(resp.Body)
	s := string(body)

	// Find an item ID
	var itemID string
	searchFrom := 0
	for {
		idx := strings.Index(s[searchFrom:], `href="/views/notes/`)
		if idx == -1 {
			break
		}
		pos := searchFrom + idx + len(`href="/views/notes/`)
		end := strings.Index(s[pos:], `"`)
		candidate := s[pos : pos+end]
		if candidate != "new" && candidate != "stats" && !strings.HasPrefix(candidate, "edit/") && !strings.HasPrefix(candidate, "delete/") {
			itemID = candidate
			break
		}
		searchFrom = pos + end
	}
	if itemID == "" {
		t.Skip("No item links found in notes list")
	}

	// Call the new API endpoint
	resp, err = http.Get(srv.URL + "/api/get/" + itemID)
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}

	body, _ = io.ReadAll(resp.Body)
	var result map[string]interface{}
	if err := json.Unmarshal(body, &result); err != nil {
		t.Fatalf("invalid JSON response: %v. Body: %s", err, string(body))
	}

	// Check structure
	if _, ok := result["frontmatter"]; !ok {
		t.Error("expected 'frontmatter' key in response")
	}
	if _, ok := result["body"]; !ok {
		t.Error("expected 'body' key in response")
	}

	// Check frontmatter has id
	fm := result["frontmatter"].(map[string]interface{})
	if id, ok := fm["id"].(string); !ok || id != itemID {
		t.Errorf("expected frontmatter.id to be %s, got %v", itemID, id)
	}
}

func TestAPIGetItem_NotFound(t *testing.T) {
	srv, tmpDir := setupTestServer(t)
	defer srv.Close()
	defer os.RemoveAll(tmpDir)

	resp, err := http.Get(srv.URL + "/api/get/nonexistent-id-12345")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 404 {
		t.Errorf("expected 404, got %d", resp.StatusCode)
	}

	body, _ := io.ReadAll(resp.Body)
	var result map[string]string
	if err := json.Unmarshal(body, &result); err != nil {
		t.Fatalf("invalid JSON response: %v", err)
	}
	if result["error"] != "item not found" {
		t.Errorf("expected error message 'item not found', got %s", result["error"])
	}
}

func TestImportCLI(t *testing.T) {
	// Create source files to import
	sourceDir := t.TempDir()
	exoDir := t.TempDir()

	sourceFile := filepath.Join(sourceDir, "hello.md")
	content := "---\ntitle: Hello World\n---\n# Hello World\nThis is a test."
	if err := os.WriteFile(sourceFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	// Run the CLI
	cmd := exec.Command("go", "run", ".", "import", sourceDir, "note")
	cmd.Env = append(os.Environ(), "EXO_DIR="+exoDir)
	
	output, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("command failed: %v\nOutput:\n%s", err, string(output))
	}

	// Verify output
	outStr := string(output)
	if !strings.Contains(outStr, "Imported: 1") {
		t.Errorf("expected output to contain 'Imported: 1', got:\n%s", outStr)
	}

	// Verify imported file exists in exoDir
	found := false
	filepath.Walk(exoDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if !info.IsDir() && filepath.Ext(path) == ".md" {
			found = true
			data, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			if !strings.Contains(string(data), "title: Hello World") {
				t.Errorf("imported file content incorrect: %s", string(data))
			}
		}
		return nil
	})

	if !found {
		t.Error("expected imported file to be written to exoDir")
	}
}
func TestWikiLinks(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "exo-test-*")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(tmpDir)

	// Copy example-repo config
	cmd := exec.Command("cp", "-a", "./example-repo/.", tmpDir)
	if err := cmd.Run(); err != nil {
		t.Fatal(err)
	}

	// Write our custom test files to tmpDir before Cache/Handlers initialization
	noteADir := filepath.Join(tmpDir, "note", "2026", "06")
	if err := os.MkdirAll(noteADir, 0755); err != nil {
		t.Fatal(err)
	}

	// Note A (the target)
	noteAPath := filepath.Join(noteADir, "itema.md")
	noteAContent := `---
id: itema
type: note
title: "Linked Note A"
created: 2026-06-30
tags: []
---
Body A`
	if err := os.WriteFile(noteAPath, []byte(noteAContent), 0644); err != nil {
		t.Fatal(err)
	}

	// Note B (the linker)
	noteBPath := filepath.Join(noteADir, "itemb.md")
	noteBContent := `---
id: itemb
type: note
title: "Note B"
created: 2026-06-30
tags: []
---
This is a link to [[itema]] and a broken link to [[nonexistent]].`
	if err := os.WriteFile(noteBPath, []byte(noteBContent), 0644); err != nil {
		t.Fatal(err)
	}

	cfg, err := config.Load(tmpDir)
	if err != nil {
		t.Fatal(err)
	}

	c, err := cache.New(tmpDir)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()

	r := repo.New(tmpDir, c)
	h, err := handlers.New(cfg, tmpDir, r, c, os.DirFS("."))
	if err != nil {
		t.Fatal(err)
	}

	mux := http.NewServeMux()
	mux.HandleFunc("GET /views/{viewId}/{itemId}", h.ViewDetail)

	srv := httptest.NewServer(mux)
	defer srv.Close()

	resp, err := http.Get(srv.URL + "/views/notes/itemb")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
	body, _ := io.ReadAll(resp.Body)
	html := string(body)

	// Assert linked item A is rendered correctly as an HTML link with title
	assertContains(t, html, `<a href="/views/notes/itema" rel="nofollow">Linked Note A</a>`)
	// Assert broken link remains unrendered / kept as [[nonexistent]]
	assertContains(t, html, `[[nonexistent]]`)
}


func assertContains(t *testing.T, body, substr string) {
	t.Helper()
	if !strings.Contains(body, substr) {
		t.Errorf("expected body to contain %q, but it didn't (body length: %d)", substr, len(body))
	}
}
