package main

import (
	"embed"
	"fmt"
	"io/fs"
	"log"
	"net/http"
	"os"
	"path/filepath"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/exporter"
	"github.com/gnur/exokephalos/internal/handlers"
	"github.com/gnur/exokephalos/internal/importer"
	"github.com/gnur/exokephalos/internal/lsp"
	"github.com/gnur/exokephalos/internal/repo"
	"github.com/gnur/exokephalos/internal/tui"
	"strings"
)

//go:embed templates/*
var templatesFS embed.FS

//go:embed static/*
var staticFS embed.FS

func main() {
	dir := os.Getenv("EXO_DIR")
	if dir == "" {
		dir = "./example-repo"
	}

	if len(os.Args) > 1 && os.Args[1] == "helix-init" {
		runHelixInit(dir)
		return
	}

	if len(os.Args) > 1 && os.Args[1] == "import" {
		runImport(dir)
		return
	}

	if len(os.Args) > 1 && os.Args[1] == "export" {
		runExport(dir)
		return
	}

	// Load configuration (required for both TUI and web)
	cfg, err := config.Load(dir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error loading configuration from %s: %v\n", dir, err)
		fmt.Fprintf(os.Stderr, "\nCreate a .exo/ directory or a .exo.toml file in your EXO_DIR to configure views.\n")
		fmt.Fprintf(os.Stderr, "See the example-repo/.exo/ for reference.\n")
		os.Exit(1)
	}

	// Initialize the in-memory cache (scans filesystem, starts watcher).
	c, err := cache.New(dir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error initializing cache: %v\n", err)
		os.Exit(1)
	}
	defer c.Close()

	r := repo.New(dir, c)

	if len(os.Args) > 1 && os.Args[1] == "serve" {
		runServer(cfg, dir, r, c)
	} else if len(os.Args) > 1 && os.Args[1] == "lsp" {
		runLSP(c)
	} else {
		if err := tui.Run(cfg, dir, c); err != nil {
			log.Fatalf("TUI error: %v", err)
		}
	}
}

func runServer(cfg *config.Config, dir string, r *repo.Repo, c *cache.Cache) {
	h, err := handlers.New(cfg, dir, r, c, templatesFS)
	if err != nil {
		log.Fatalf("Failed to initialize handlers: %v", err)
	}

	mux := http.NewServeMux()

	// Static files (served from embedded FS)
	staticSub, _ := fs.Sub(staticFS, "static")
	mux.Handle("GET /static/", http.StripPrefix("/static/", http.FileServer(http.FS(staticSub))))

	// --- API endpoints ---
	mux.HandleFunc("GET /api/get/{id}", h.GetItemByID)

	// --- Generic view routes ---
	mux.HandleFunc("GET /views/{viewId}/stats", h.ViewStats)
	mux.HandleFunc("GET /views/{viewId}/new", h.ViewNew)
	mux.HandleFunc("POST /views/{viewId}/new", h.ViewNew)
	mux.HandleFunc("GET /views/{viewId}/edit/{itemId}", h.ViewEdit)
	mux.HandleFunc("POST /views/{viewId}/edit/{itemId}", h.ViewEdit)
	mux.HandleFunc("POST /views/{viewId}/delete/{itemId}", h.ViewDelete)
	mux.HandleFunc("POST /views/{viewId}/items/{itemId}/actions/{actionName}", h.ViewAction)
	mux.HandleFunc("GET /views/{viewId}/{itemId}", h.ViewDetail)
	mux.HandleFunc("GET /views/{viewId}", h.ViewList)

	// --- Hardcoded API endpoints (not view-specific) ---
	mux.HandleFunc("POST /webhook/{source}", h.WebhookReceive)

	// Ping
	mux.HandleFunc("GET /ping", func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte("pong"))
	})

	// Root redirect to default view
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

	// Start HTTP server
	fmt.Println("Exokephalos listening on :8293")
	log.Fatal(http.ListenAndServe(":8293", h.TimingMiddleware(h.CSRFMiddleware(mux))))
}

func runLSP(c *cache.Cache) {
	if err := lsp.RunServer(c); err != nil {
		log.Fatalf("LSP error: %v", err)
	}
}

func runHelixInit(dir string) {
	helixDir := filepath.Join(dir, ".helix")
	if err := os.MkdirAll(helixDir, 0755); err != nil {
		fmt.Fprintf(os.Stderr, "Error creating .helix directory: %v\n", err)
		os.Exit(1)
	}

	configPath := filepath.Join(helixDir, "languages.toml")
	configContent := `[[language]]
name = "markdown"
language-servers = ["exokephalos"]

[language-server.exokephalos]
command = "exo"
args = ["lsp"]
`

	if err := os.WriteFile(configPath, []byte(configContent), 0644); err != nil {
		fmt.Fprintf(os.Stderr, "Error writing languages.toml: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Created %s\n", configPath)
	fmt.Println("Helix will now use exo lsp for markdown files in this directory.")
}

func runImport(exoDir string) {
	if len(os.Args) < 4 {
		fmt.Fprintf(os.Stderr, "Usage: exo import <source-dir> <type>\n")
		fmt.Fprintf(os.Stderr, "\nRecursively imports markdown files from source-dir into EXO_DIR.\n")
		fmt.Fprintf(os.Stderr, "Files are placed in EXO_DIR/<first-3-id-chars>/<id>.md\n")
		os.Exit(1)
	}

	sourceDir := os.Args[2]
	typ := os.Args[3]

	// Verify source directory exists
	if info, err := os.Stat(sourceDir); err != nil || !info.IsDir() {
		fmt.Fprintf(os.Stderr, "Error: source directory does not exist: %s\n", sourceDir)
		os.Exit(1)
	}

	// Ensure EXO_DIR exists
	if err := os.MkdirAll(exoDir, 0755); err != nil {
		fmt.Fprintf(os.Stderr, "Error creating EXO_DIR: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Importing from %s into %s (type: %s)\n", sourceDir, exoDir, typ)

	result := importer.Import(sourceDir, exoDir, typ)

	fmt.Printf("\nImport complete:\n")
	fmt.Printf("  Imported: %d\n", result.Imported)
	fmt.Printf("  Skipped:  %d\n", result.Skipped)

	if len(result.Errors) > 0 {
		fmt.Printf("\nErrors/Warnings:\n")
		for _, err := range result.Errors {
			fmt.Printf("  - %s\n", err)
		}
	}
}

func runExport(exoDir string) {
	var outputDir string
	var targetType string

	for i := 2; i < len(os.Args); i++ {
		if os.Args[i] == "--type" && i+1 < len(os.Args) {
			targetType = os.Args[i+1]
			i++
		} else if strings.HasPrefix(os.Args[i], "--type=") {
			targetType = strings.TrimPrefix(os.Args[i], "--type=")
		} else {
			outputDir = os.Args[i]
		}
	}

	if outputDir == "" {
		fmt.Fprintf(os.Stderr, "Usage: exo export <output-dir> [--type <type>]\n")
		fmt.Fprintf(os.Stderr, "\nExports markdown files from EXO_DIR into the output directory.\n")
		fmt.Fprintf(os.Stderr, "Files are placed in <output-dir>/<type>/<year>/<month>/<slug-title>.md\n")
		os.Exit(1)
	}

	// Initialize cache (required to load items)
	c, err := cache.New(exoDir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error initializing cache: %v\n", err)
		os.Exit(1)
	}
	defer c.Close()

	opts := exporter.ExportOptions{
		OutputDir:  outputDir,
		TargetType: targetType,
	}

	fmt.Printf("Exporting from %s into %s", exoDir, outputDir)
	if targetType != "" {
		fmt.Printf(" (type: %s)", targetType)
	}
	fmt.Println()

	result := exporter.Export(c, opts)

	fmt.Printf("\nExport complete:\n")
	fmt.Printf("  Exported: %d\n", result.Exported)

	if len(result.Errors) > 0 {
		fmt.Printf("\nErrors/Warnings:\n")
		for _, err := range result.Errors {
			fmt.Printf("  - %s\n", err)
		}
	}
}
