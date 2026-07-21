//go:build e2e

package e2e

import (
	"bufio"
	"bytes"
	"context"
	"database/sql"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/creack/pty"
	_ "modernc.org/sqlite"
)

const clientID = "e2e-tui-client"

type harness struct {
	t         *testing.T
	root      string
	bin       string
	serverDir string
	clientDir string
	baseURL   string
	password  string

	serverCmd *exec.Cmd
	serverOut safeBuffer
	tuiCmd    *exec.Cmd
	tuiPTY    *os.File
	tuiOut    safeBuffer
}

type safeBuffer struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (b *safeBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.b.Write(p)
}

func (b *safeBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.b.String()
}

func TestTUISPASyncEndToEnd(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("PTY-driven TUI E2E is not supported on Windows")
	}
	h := newHarness(t)
	h.buildBinary()
	h.seedWorkspaces()
	h.startServer()
	h.startTUI()
	h.startSyncFromTUI()

	h.runPlaywright("approve-and-exercise-spa")
	h.waitTUI("✓", 20*time.Second)
	h.waitForOutboxSynced(30 * time.Second)
	h.assertServerHasUploadedState()

	h.writeClientNoteUpdate()
	h.runStartSyncAction()
	h.waitServerBodyContains("Updated from the TUI workspace", 75*time.Second)

	first := h.configOutboxCount()
	time.Sleep(11 * time.Second)
	second := h.configOutboxCount()
	if second != first {
		t.Fatalf("config outbox rows changed across reconcile ticks: before=%d after=%d", first, second)
	}
}

func newHarness(t *testing.T) *harness {
	t.Helper()
	root, err := filepath.Abs("..")
	if err != nil {
		t.Fatal(err)
	}
	tmp := t.TempDir()
	h := &harness{
		t:         t,
		root:      root,
		bin:       filepath.Join(tmp, "xo"),
		serverDir: filepath.Join(tmp, "server"),
		clientDir: filepath.Join(tmp, "client"),
	}
	t.Cleanup(h.cleanup)
	return h
}

func (h *harness) buildBinary() {
	h.t.Helper()
	webCmd := exec.Command("npm", "run", "build:web")
	webCmd.Dir = h.root
	if out, err := webCmd.CombinedOutput(); err != nil {
		h.t.Fatalf("build SPA assets: %v\n%s", err, out)
	}
	version := "e2e"
	buildTime := time.Now().UTC().Format(time.RFC3339)
	ldflags := fmt.Sprintf("-s -w -X github.com/gnur/exokephalos/internal/version.Version=%s -X github.com/gnur/exokephalos/internal/version.BuildTime=%s", version, buildTime)
	cmd := exec.Command("go", "build", "-ldflags="+ldflags, "-o", h.bin, ".")
	cmd.Dir = h.root
	out, err := cmd.CombinedOutput()
	if err != nil {
		h.t.Fatalf("build xo: %v\n%s", err, out)
	}
}

func (h *harness) seedWorkspaces() {
	h.t.Helper()
	mustMkdir(h.t, filepath.Join(h.serverDir, ".exo"))
	mustMkdir(h.t, filepath.Join(h.clientDir, ".exo"))

	for _, dir := range []string{h.serverDir, h.clientDir} {
		writeFile(h.t, filepath.Join(dir, "exo.fnl"), luaWorkspaceConfig)
		writeFile(h.t, filepath.Join(dir, "modules", "actions.lua"), luaActionsModule)
	}
	writeFile(h.t, filepath.Join(h.clientDir, "note", "2026", "06", "test.md"), `---
id: e2enote
type: note
tags: [todo]
created: 2026-06-01T10:00:00Z
title: E2E Seed Note
---

# E2E Seed Note

This note starts on the TUI client.
`)
	writeFile(h.t, filepath.Join(h.clientDir, "docs", "intro.md"), `---
id: e2edoc
type: doc
tags: []
created: 2026-06-01T10:05:00Z
title: E2E Doc
---

# E2E Doc

This doc verifies config-backed view sync.
`)

	port := freePort(h.t)
	h.baseURL = fmt.Sprintf("http://127.0.0.1:%d", port)
	writeFile(h.t, filepath.Join(h.serverDir, ".exo", "serve.fnl"), fmt.Sprintf(`{:server {:db-path ".exo/server.sqlite" :listen "127.0.0.1:%d"}}`, port))
	writeFile(h.t, filepath.Join(h.clientDir, ".exo", "tui.fnl"), fmt.Sprintf(`{:sync {:server-url %q :client-id %q}}`, h.baseURL, clientID))
}

const luaWorkspaceConfig = `(local actions (require :modules.actions))
{:default-view :notes
 :views {:notes {:name "Notes" :key "n" :show-tags true
                 :when (fn [note] (= note.type "note"))
                 :subviews [{:name "All" :when (fn [_] true)}]}
         :docs {:name "Docs" :key "d" :show-tags true
                :when (fn [note] (= note.type "doc"))
                :subviews [{:name "All" :when (fn [_] true)}]}}
 :actions actions}`

const luaActionsModule = `return {
  ["mark-done"] = {
    description = "Mark item as done",
    when = function(note)
      return has_tag(note.tags, "todo") and not has_tag(note.tags, "done")
    end,
    run = function(note)
      note.tags = add_tag(remove_tag(note.tags, "todo"), "done")
      note.status = "done"
      note.completed_at = now()
      return note
    end,
  },
}`

func (h *harness) startServer() {
	h.t.Helper()
	ctx := context.Background()
	cmd := exec.CommandContext(ctx, h.bin, "serve")
	cmd.Dir = h.root
	cmd.Env = append(os.Environ(), "EXO_DIR="+h.serverDir)
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		h.t.Fatal(err)
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		h.t.Fatal(err)
	}
	if err := cmd.Start(); err != nil {
		h.t.Fatalf("start server: %v", err)
	}
	h.serverCmd = cmd
	go h.captureServer(stdout)
	go io.Copy(&h.serverOut, stderr)

	h.password = h.waitPassword(15 * time.Second)
	h.waitHTTP("/ping", 15*time.Second)
}

func (h *harness) captureServer(r io.Reader) {
	scanner := bufio.NewScanner(r)
	for scanner.Scan() {
		line := scanner.Text() + "\n"
		_, _ = h.serverOut.Write([]byte(line))
	}
}

func (h *harness) waitPassword(timeout time.Duration) string {
	jsonRe := regexp.MustCompile(`"password":"([^"]+)"`)
	plainRe := regexp.MustCompile(`initial web password:\s+(\S+)`)
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		output := h.serverOut.String()
		if match := jsonRe.FindStringSubmatch(output); len(match) == 2 {
			return match[1]
		}
		if match := plainRe.FindStringSubmatch(output); len(match) == 2 {
			return match[1]
		}
		time.Sleep(100 * time.Millisecond)
	}
	h.t.Fatalf("server did not print initial password; output:\n%s", h.serverOut.String())
	return ""
}

func (h *harness) waitHTTP(path string, timeout time.Duration) {
	h.t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		resp, err := http.Get(h.baseURL + path)
		if err == nil {
			_ = resp.Body.Close()
			if resp.StatusCode < 500 {
				return
			}
		}
		time.Sleep(150 * time.Millisecond)
	}
	h.t.Fatalf("server did not respond at %s%s; output:\n%s", h.baseURL, path, h.serverOut.String())
}

func (h *harness) startTUI() {
	h.t.Helper()
	cmd := exec.Command(h.bin)
	cmd.Dir = h.root
	cmd.Env = append(os.Environ(), "EXO_DIR="+h.clientDir, "TERM=xterm-256color")
	f, err := pty.StartWithSize(cmd, &pty.Winsize{Rows: 32, Cols: 120})
	if err != nil {
		h.t.Fatalf("start TUI: %v", err)
	}
	h.tuiCmd = cmd
	h.tuiPTY = f
	go io.Copy(&h.tuiOut, f)
	h.waitTUI("sync: not started", 10*time.Second)
}

func (h *harness) startSyncFromTUI() {
	h.t.Helper()
	h.runStartSyncAction()
	h.waitTUI("pending approval", 15*time.Second)
}

func (h *harness) runStartSyncAction() {
	h.t.Helper()
	if _, err := h.tuiPTY.Write([]byte(":")); err != nil {
		h.t.Fatalf("open action picker: %v", err)
	}
	h.waitTUI("start-sync", 5*time.Second)
	if _, err := h.tuiPTY.Write([]byte("start-sync\r")); err != nil {
		h.t.Fatalf("send start-sync action: %v", err)
	}
}

func (h *harness) waitTUI(needle string, timeout time.Duration) {
	h.t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		out := stripANSI(h.tuiOut.String())
		if strings.Contains(out, needle) {
			return
		}
		if strings.Contains(strings.ToLower(out), "panic") {
			h.t.Fatalf("TUI panicked while waiting for %q:\n%s", needle, out)
		}
		time.Sleep(100 * time.Millisecond)
	}
	h.t.Fatalf("TUI did not show %q; output:\n%s", needle, stripANSI(h.tuiOut.String()))
}

func (h *harness) runPlaywright(project string) {
	h.t.Helper()
	cmd := exec.Command("npx", "playwright", "test", "e2e/spa.spec.ts", "--project", project)
	cmd.Dir = h.root
	cmd.Env = append(os.Environ(),
		"EXO_E2E_BASE_URL="+h.baseURL,
		"EXO_E2E_PASSWORD="+h.password,
		"EXO_E2E_CLIENT_ID="+clientID,
	)
	out, err := cmd.CombinedOutput()
	if err != nil {
		h.t.Fatalf("playwright: %v\n%s", err, out)
	}
}

func (h *harness) waitForOutboxSynced(timeout time.Duration) {
	h.t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if h.outboxCountWhere("status != 'synced'") == 0 && h.outboxCountWhere("status = 'synced'") > 0 {
			return
		}
		time.Sleep(250 * time.Millisecond)
	}
	h.t.Fatalf("outbox did not drain; rows=%s", h.outboxSummary())
}

func (h *harness) assertServerHasUploadedState() {
	db := openDB(h.t, filepath.Join(h.serverDir, ".exo", "server.sqlite"))
	defer db.Close()
	var items, configs int
	if err := db.QueryRow(`SELECT count(*) FROM items WHERE deleted_at = ''`).Scan(&items); err != nil {
		h.t.Fatal(err)
	}
	if err := db.QueryRow(`SELECT count(*) FROM configs WHERE deleted_at = ''`).Scan(&configs); err != nil {
		h.t.Fatal(err)
	}
	if items == 0 || configs == 0 {
		h.t.Fatalf("server upload incomplete: items=%d configs=%d", items, configs)
	}
}

func (h *harness) writeClientNoteUpdate() {
	h.t.Helper()
	path := filepath.Join(h.clientDir, "note", "2026", "06", "test.md")
	f, err := os.OpenFile(path, os.O_APPEND|os.O_WRONLY, 0644)
	if err != nil {
		h.t.Fatalf("open note for update: %v", err)
	}
	defer f.Close()
	if _, err := f.WriteString("\nUpdated from the TUI workspace.\n"); err != nil {
		h.t.Fatalf("append note update: %v", err)
	}
}

func (h *harness) waitServerBodyContains(needle string, timeout time.Duration) {
	h.t.Helper()
	dbPath := filepath.Join(h.serverDir, ".exo", "server.sqlite")
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		db, err := sql.Open("sqlite", dbPath)
		if err == nil {
			var count int
			err = db.QueryRow(`SELECT count(*) FROM items WHERE body LIKE ? AND deleted_at = ''`, "%"+needle+"%").Scan(&count)
			_ = db.Close()
			if err == nil && count > 0 {
				return
			}
		}
		time.Sleep(500 * time.Millisecond)
	}
	h.t.Fatalf("server never received updated note body containing %q; TUI output:\n%s", needle, stripANSI(h.tuiOut.String()))
}

func (h *harness) configOutboxCount() int {
	return h.outboxCountWhere("target_kind = 'config'")
}

func (h *harness) outboxCountWhere(where string) int {
	db := openDB(h.t, filepath.Join(h.clientDir, ".exo", "cache.sqlite"))
	defer db.Close()
	var count int
	if err := db.QueryRow(`SELECT count(*) FROM outbox WHERE ` + where).Scan(&count); err != nil {
		h.t.Fatal(err)
	}
	return count
}

func (h *harness) outboxSummary() string {
	db := openDB(h.t, filepath.Join(h.clientDir, ".exo", "cache.sqlite"))
	defer db.Close()
	rows, err := db.Query(`SELECT status, target_kind, count(*) FROM outbox GROUP BY status, target_kind ORDER BY status, target_kind`)
	if err != nil {
		return err.Error()
	}
	defer rows.Close()
	var parts []string
	for rows.Next() {
		var status, kind string
		var count int
		_ = rows.Scan(&status, &kind, &count)
		parts = append(parts, fmt.Sprintf("%s/%s=%d", status, kind, count))
	}
	return strings.Join(parts, ", ")
}

func (h *harness) cleanup() {
	if h.tuiPTY != nil {
		_, _ = h.tuiPTY.Write([]byte("q"))
		_ = h.tuiPTY.Close()
	}
	if h.tuiCmd != nil && h.tuiCmd.Process != nil {
		_ = h.tuiCmd.Process.Kill()
		_, _ = h.tuiCmd.Process.Wait()
	}
	if h.serverCmd != nil && h.serverCmd.Process != nil {
		_ = h.serverCmd.Process.Kill()
		_, _ = h.serverCmd.Process.Wait()
	}
}

func freePort(t *testing.T) int {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	return ln.Addr().(*net.TCPAddr).Port
}

func copyTree(t *testing.T, src, dst string) {
	t.Helper()
	err := filepath.WalkDir(src, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, err := filepath.Rel(src, path)
		if err != nil {
			return err
		}
		target := filepath.Join(dst, rel)
		if d.IsDir() {
			return os.MkdirAll(target, 0755)
		}
		copyFile(t, path, target)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
}

func copyFile(t *testing.T, src, dst string) {
	t.Helper()
	data, err := os.ReadFile(src)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(dst), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(dst, data, 0644); err != nil {
		t.Fatal(err)
	}
}

func writeFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}
}

func mustMkdir(t *testing.T, path string) {
	t.Helper()
	if err := os.MkdirAll(path, 0755); err != nil {
		t.Fatal(err)
	}
}

func openDB(t *testing.T, path string) *sql.DB {
	t.Helper()
	db, err := sql.Open("sqlite", path)
	if err != nil {
		t.Fatal(err)
	}
	if err := db.Ping(); err != nil {
		_ = db.Close()
		t.Fatal(err)
	}
	return db
}

func stripANSI(s string) string {
	re := regexp.MustCompile(`\x1b\[[0-9;?]*[ -/]*[@-~]`)
	return re.ReplaceAllString(s, "")
}
