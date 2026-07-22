package main

import (
	"fmt"
	"io"
	"math/rand"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"os/exec"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/handlers"
	"github.com/gnur/exokephalos/internal/repo"
)

// Tag pool based on example-repo note tags
var benchTags = []string{
	"idea", "k8s", "todo", "prive", "recept",
	"scratch", "tfq", "architecture", "fleetcontrol", "project",
}

// Subviews available for querying
var benchSubviews = []string{"All", "Todo", "Recipes"}

// randomTags returns 1-3 random tags from the pool.
func randomTags(rng *rand.Rand) []string {
	n := rng.Intn(3) + 1
	picked := make(map[int]bool)
	tags := make([]string, 0, n)
	for len(tags) < n {
		idx := rng.Intn(len(benchTags))
		if !picked[idx] {
			picked[idx] = true
			tags = append(tags, benchTags[idx])
		}
	}
	return tags
}

// randomQueryTags returns 1-2 random tags for query filtering.
func randomQueryTags(rng *rand.Rand) []string {
	n := rng.Intn(2) + 1
	picked := make(map[int]bool)
	tags := make([]string, 0, n)
	for len(tags) < n {
		idx := rng.Intn(len(benchTags))
		if !picked[idx] {
			picked[idx] = true
			tags = append(tags, benchTags[idx])
		}
	}
	return tags
}

// generateTitle creates a random note title.
func generateTitle(rng *rand.Rand, index int) string {
	words := []string{
		"benchmark", "test", "performance", "analysis", "review",
		"design", "architecture", "proposal", "research", "spike",
		"migration", "refactor", "feature", "bugfix", "optimization",
		"deployment", "monitoring", "scaling", "security", "automation",
	}
	w1 := words[rng.Intn(len(words))]
	w2 := words[rng.Intn(len(words))]
	return fmt.Sprintf("%s %s %d", w1, w2, index)
}

// setupBenchServer creates a test server with a temp directory from the example-repo.
// It accepts testing.TB so it works with both *testing.T and *testing.B.
func setupBenchServer(tb testing.TB) (*httptest.Server, string, *cache.Cache) {
	tb.Helper()

	tmpDir, err := os.MkdirTemp("", "exo-bench-*")
	if err != nil {
		tb.Fatalf("Failed to create temp dir: %v", err)
	}

	// Copy only the workspace config from example-repo.
	cmd := exec.Command("cp", "-a", "./example-repo/exo.fnl", tmpDir+"/exo.fnl")
	if err := cmd.Run(); err != nil {
		os.RemoveAll(tmpDir)
		tb.Fatalf("Failed to copy config: %v", err)
	}

	// Create the zettelkasten directory
	if err := os.MkdirAll(tmpDir+"/zettelkasten", 0755); err != nil {
		os.RemoveAll(tmpDir)
		tb.Fatalf("Failed to create zettelkasten dir: %v", err)
	}

	cfg, err := config.Load(tmpDir)
	if err != nil {
		os.RemoveAll(tmpDir)
		tb.Fatalf("Failed to load config: %v", err)
	}

	c, err := cache.New(tmpDir)
	if err != nil {
		os.RemoveAll(tmpDir)
		tb.Fatalf("Failed to create cache: %v", err)
	}

	r := repo.New(tmpDir, c)
	h, err := handlers.New(cfg, tmpDir, r, c, os.DirFS("."))
	if err != nil {
		c.Close()
		os.RemoveAll(tmpDir)
		tb.Fatalf("Failed to create handlers: %v", err)
	}

	mux := http.NewServeMux()
	mux.Handle("GET /static/", http.StripPrefix("/static/", http.FileServer(http.Dir("static"))))
	mux.HandleFunc("GET /views/{viewId}/stats", h.ViewStats)
	mux.HandleFunc("GET /views/{viewId}/new", h.ViewNew)
	mux.HandleFunc("POST /views/{viewId}/new", h.ViewNew)
	mux.HandleFunc("GET /views/{viewId}/edit/{itemId}", h.ViewEdit)
	mux.HandleFunc("POST /views/{viewId}/edit/{itemId}", h.ViewEdit)
	mux.HandleFunc("POST /views/{viewId}/delete/{itemId}", h.ViewDelete)
	mux.HandleFunc("GET /views/{viewId}/{itemId}", h.ViewDetail)
	mux.HandleFunc("GET /views/{viewId}", h.ViewList)
	mux.HandleFunc("POST /webhook/{source}", h.WebhookReceive)

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

	return httptest.NewServer(h.TimingMiddleware(h.CSRFMiddleware(mux))), tmpDir, c
}

// populateNotes writes numNotes notes directly to the filesystem with random tags.
func populateNotes(tb testing.TB, tmpDir string, numNotes int) {
	tb.Helper()

	rng := rand.New(rand.NewSource(42)) // Fixed seed for reproducibility

	tb.Logf("Creating %d notes on filesystem...", numNotes)
	start := time.Now()

	for i := 0; i < numNotes; i++ {
		id := fmt.Sprintf("%05x", i)
		title := generateTitle(rng, i)
		tags := randomTags(rng)
		tagStr := strings.Join(tags, ", ")
		date := time.Now().AddDate(0, 0, -rng.Intn(365)).Format("2006-01-02")

		slug := strings.ReplaceAll(strings.ToLower(title), " ", "-")
		filename := fmt.Sprintf("zettelkasten/%s-%s.md", id, slug)

		content := fmt.Sprintf(`---
type: note
tags: [%s]
id: %s
created: %s
title: "%s"
---

# %s

This is benchmark note %d with tags [%s].
Some content to make the file more realistic.
Lorem ipsum dolor sit amet, consectetur adipiscing elit.
Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.
`, tagStr, id, date, title, title, i, tagStr)

		path := tmpDir + "/" + filename
		if err := os.WriteFile(path, []byte(content), 0644); err != nil {
			tb.Fatalf("Failed to write note %d: %v", i, err)
		}
	}

	tb.Logf("Created %d notes in %.1fs", numNotes, time.Since(start).Seconds())
}

// latencyRecorder tracks request latencies for percentile reporting.
type latencyRecorder struct {
	mu        sync.Mutex
	latencies []time.Duration
}

func (r *latencyRecorder) record(d time.Duration) {
	r.mu.Lock()
	r.latencies = append(r.latencies, d)
	r.mu.Unlock()
}

func (r *latencyRecorder) report(tb testing.TB) {
	r.mu.Lock()
	defer r.mu.Unlock()

	if len(r.latencies) == 0 {
		tb.Log("  No latencies recorded")
		return
	}

	sort.Slice(r.latencies, func(i, j int) bool {
		return r.latencies[i] < r.latencies[j]
	})

	n := len(r.latencies)
	var total time.Duration
	for _, l := range r.latencies {
		total += l
	}

	tb.Logf("  n=%d  avg=%v  p50=%v  p90=%v  p95=%v  p99=%v  max=%v",
		n,
		total/time.Duration(n),
		r.latencies[n*50/100],
		r.latencies[n*90/100],
		r.latencies[n*95/100],
		r.latencies[n*99/100],
		r.latencies[n-1],
	)
}

// BenchmarkNoteQueries benchmarks concurrent tag queries against a pool of 2000 notes.
//
// Run with:
//
//	go test -bench=BenchmarkNoteQueries -benchtime=1000x -cpu=1,2,4,8 -v
func BenchmarkNoteQueries(b *testing.B) {
	srv, tmpDir, c := setupBenchServer(b)
	defer srv.Close()
	defer c.Close()
	defer os.RemoveAll(tmpDir)

	const numNotes = 2000
	populateNotes(b, tmpDir, numNotes)

	// Sync cache so it picks up all notes written directly to disk.
	if err := c.Sync(); err != nil {
		b.Fatalf("Failed to sync cache: %v", err)
	}

	// Verify notes are accessible via the notes view
	resp, err := http.Get(srv.URL + "/views/notes")
	if err != nil {
		b.Fatal(err)
	}
	body, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != 200 {
		b.Fatalf("Failed to list notes: status %d", resp.StatusCode)
	}
	b.Logf("Notes view loaded OK, response size: %d bytes", len(body))

	// Track metrics
	var queryCount, errorCount atomic.Int64
	queryLatency := &latencyRecorder{}

	b.ResetTimer()
	b.RunParallel(func(pb *testing.PB) {
		client := &http.Client{}
		rng := rand.New(rand.NewSource(time.Now().UnixNano() + rand.Int63()))

		for pb.Next() {
			doQuery(rng, client, srv.URL, queryLatency, &queryCount, &errorCount)
		}
	})
	b.StopTimer()

	b.Logf("")
	b.Logf("=== Benchmark Results ===")
	b.Logf("Queries:  %d", queryCount.Load())
	b.Logf("Errors:   %d", errorCount.Load())
	b.Logf("")
	b.Logf("--- Query Latency ---")
	queryLatency.report(b)
}

// BenchmarkNoteCreation benchmarks note creation via the HTTP API.
//
// Run with:
//
//	go test -bench=BenchmarkNoteCreation -benchtime=500x -v
func BenchmarkNoteCreation(b *testing.B) {
	srv, tmpDir, c := setupBenchServer(b)
	defer srv.Close()
	defer c.Close()
	defer os.RemoveAll(tmpDir)

	var created atomic.Int64
	createLatency := &latencyRecorder{}

	b.ResetTimer()
	b.RunParallel(func(pb *testing.PB) {
		client := &http.Client{
			CheckRedirect: func(req *http.Request, via []*http.Request) error {
				return http.ErrUseLastResponse
			},
		}
		rng := rand.New(rand.NewSource(time.Now().UnixNano() + rand.Int63()))

		for pb.Next() {
			title := generateTitle(rng, int(created.Add(1)))
			form := url.Values{}
			form.Set("Title", title)

			start := time.Now()
			resp, err := client.PostForm(srv.URL+"/views/notes/new", form)
			elapsed := time.Since(start)

			if err != nil {
				continue
			}
			io.ReadAll(resp.Body)
			resp.Body.Close()

			if resp.StatusCode == http.StatusSeeOther {
				createLatency.record(elapsed)
			}
		}
	})
	b.StopTimer()

	b.Logf("")
	b.Logf("=== Creation Benchmark Results ===")
	b.Logf("Notes created: %d", created.Load())
	b.Logf("--- Create Latency ---")
	createLatency.report(b)
}

// BenchmarkTagQueries benchmarks only tag-filtered queries with no concurrent writes.
//
// Run with:
//
//	go test -bench=BenchmarkTagQueries -benchtime=1000x -cpu=1,2,4,8 -v
func BenchmarkTagQueries(b *testing.B) {
	srv, tmpDir, c := setupBenchServer(b)
	defer srv.Close()
	defer c.Close()
	defer os.RemoveAll(tmpDir)

	const numNotes = 2000
	populateNotes(b, tmpDir, numNotes)

	// Sync cache so it picks up all notes written directly to disk.
	if err := c.Sync(); err != nil {
		b.Fatalf("Failed to sync cache: %v", err)
	}

	var queryCount atomic.Int64
	queryLatency := &latencyRecorder{}

	b.ResetTimer()
	b.RunParallel(func(pb *testing.PB) {
		client := &http.Client{}
		rng := rand.New(rand.NewSource(time.Now().UnixNano() + rand.Int63()))

		for pb.Next() {
			tags := randomQueryTags(rng)
			queryURL := srv.URL + "/views/notes?tags=" + url.QueryEscape(strings.Join(tags, ","))

			// 20% chance to use subview instead of direct tag filter
			if rng.Float64() < 0.2 {
				subview := benchSubviews[rng.Intn(len(benchSubviews))]
				queryURL = srv.URL + "/views/notes?subview=" + url.QueryEscape(subview)
			}

			start := time.Now()
			resp, err := client.Get(queryURL)
			elapsed := time.Since(start)

			if err != nil {
				continue
			}
			io.ReadAll(resp.Body)
			resp.Body.Close()

			if resp.StatusCode == 200 {
				queryLatency.record(elapsed)
				queryCount.Add(1)
			}
		}
	})
	b.StopTimer()

	b.Logf("")
	b.Logf("=== Tag Query Benchmark Results ===")
	b.Logf("Queries completed: %d", queryCount.Load())
	b.Logf("--- Query Latency ---")
	queryLatency.report(b)
}

// doQuery performs a single tag-filtered query or subview query.
func doQuery(rng *rand.Rand, client *http.Client, baseURL string,
	queryLatency *latencyRecorder, queryCount, errorCount *atomic.Int64) {

	tags := randomQueryTags(rng)
	queryURL := baseURL + "/views/notes?tags=" + url.QueryEscape(strings.Join(tags, ","))

	// 20% chance to use subview query
	if rng.Float64() < 0.2 {
		subview := benchSubviews[rng.Intn(len(benchSubviews))]
		queryURL = baseURL + "/views/notes?subview=" + url.QueryEscape(subview)
	}

	start := time.Now()
	resp, err := client.Get(queryURL)
	elapsed := time.Since(start)

	if err != nil {
		errorCount.Add(1)
		return
	}
	io.ReadAll(resp.Body)
	resp.Body.Close()

	if resp.StatusCode == 200 {
		queryLatency.record(elapsed)
		queryCount.Add(1)
	} else {
		errorCount.Add(1)
	}
}
