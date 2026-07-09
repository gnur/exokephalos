package urlimport

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/repo"
)

func TestImportCreatesNoteFromReadableHTML(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		_, _ = w.Write([]byte(`<!doctype html>
<html>
<head>
  <title>Fallback Title</title>
  <meta property="og:site_name" content="Example Site">
  <meta name="author" content="Ada Lovelace">
  <meta property="article:published_time" content="2026-07-09T10:00:00Z">
</head>
<body>
  <nav>Ignore me</nav>
  <article>
    <h1>Readable Title</h1>
    <p>This is the article body with <strong>important</strong> text.</p>
    <p><a href="/next">Relative link</a></p>
  </article>
</body>
</html>`))
	}))
	defer server.Close()

	tmpDir := t.TempDir()
	c, err := cache.New(tmpDir)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	r := repo.New(tmpDir, c)

	result, err := Import(context.Background(), r, tmpDir, server.URL, WithPrivateHosts())
	if err != nil {
		t.Fatal(err)
	}

	if result.ID == "" {
		t.Fatal("expected generated id")
	}
	if result.Frontmatter["type"] != "note" {
		t.Fatalf("type = %v, want note", result.Frontmatter["type"])
	}
	if result.Frontmatter["url"] != server.URL {
		t.Fatalf("url = %v, want %s", result.Frontmatter["url"], server.URL)
	}
	if result.Frontmatter["source"] != "url" {
		t.Fatalf("source = %v, want url", result.Frontmatter["source"])
	}
	if result.Frontmatter["title"] == "" {
		t.Fatal("expected title")
	}
	if !strings.Contains(result.Body, "# ") {
		t.Fatalf("expected markdown heading in body:\n%s", result.Body)
	}
	if !strings.Contains(result.Body, "important") {
		t.Fatalf("expected converted article body:\n%s", result.Body)
	}

	fm, body, err := markdown.ParseFrontmatter(result.Path)
	if err != nil {
		t.Fatal(err)
	}
	if fm["id"] != result.ID {
		t.Fatalf("persisted id = %v, want %s", fm["id"], result.ID)
	}
	if body != result.Body {
		t.Fatal("persisted body differs from result body")
	}
}

func TestImportRejectsNonHTMLContent(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"ok": true}`))
	}))
	defer server.Close()

	tmpDir := t.TempDir()
	c, err := cache.New(tmpDir)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	r := repo.New(tmpDir, c)

	_, err = Import(context.Background(), r, tmpDir, server.URL, WithPrivateHosts())
	if err == nil {
		t.Fatal("expected non-HTML content type to fail")
	}
}

func TestImportRejectsUnsupportedScheme(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := cache.New(tmpDir)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	r := repo.New(tmpDir, c)

	_, err = Import(context.Background(), r, tmpDir, "file:///etc/passwd")
	if err == nil {
		t.Fatal("expected unsupported scheme to fail")
	}
}
