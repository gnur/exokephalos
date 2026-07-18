package main

import (
	"embed"
	"fmt"
	"io/fs"
	"log"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"time"

	"github.com/gnur/exokephalos/internal/auth"
	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/exporter"
	"github.com/gnur/exokephalos/internal/handlers"
	"github.com/gnur/exokephalos/internal/importer"
	"github.com/gnur/exokephalos/internal/lsp"
	"github.com/gnur/exokephalos/internal/syncsvc"
	"github.com/gnur/exokephalos/internal/tui"
	"github.com/gnur/exokephalos/internal/version"
	"strings"
)

//go:embed templates/*
var templatesFS embed.FS

//go:embed static/*
var staticFS embed.FS

//go:embed web/dist
var spaFS embed.FS

func main() {
	dir := os.Getenv("EXO_DIR")
	if dir == "" {
		dir = "./example-repo"
	}

	if len(os.Args) > 1 && (os.Args[1] == "version" || os.Args[1] == "--version") {
		fmt.Println(version.String())
		return
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

	appMode := "tui"
	if len(os.Args) > 1 && os.Args[1] == "serve" {
		appMode = "serve"
	}
	appCfg, err := config.LoadApp(dir, appMode)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error loading app configuration from %s: %v\n", dir, err)
		os.Exit(1)
	}

	if len(os.Args) > 1 && os.Args[1] == "serve" {
		runServer(appCfg, dir)
		return
	}

	// Load configuration (required for local TUI and LSP).
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

	if len(os.Args) > 1 && os.Args[1] == "lsp" {
		runLSP(c)
	} else {
		if err := tui.Run(cfg, dir, c, appCfg); err != nil {
			log.Fatalf("TUI error: %v", err)
		}
	}
}

func runServer(appCfg *config.AppConfig, dir string) {
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{Level: slog.LevelInfo})))

	s, err := syncsvc.NewServer(appCfg.Server.DBPath)
	if err != nil {
		slog.Error("failed to initialize sync server", "error", err)
		os.Exit(1)
	}
	defer s.Close()
	s.SetBaseDir(dir)

	cfg, err := syncsvc.LoadConfigFromServerDB(appCfg.Server.DBPath)
	if err != nil {
		slog.Error("failed to load sync server config", "error", err)
		os.Exit(1)
	}
	h, err := handlers.NewSyncServer(cfg, dir, s, templatesFS)
	if err != nil {
		slog.Error("failed to initialize handlers", "error", err)
		os.Exit(1)
	}
	authMgr, err := initAuth(appCfg.Server.DBPath, filepath.Join(dir, ".exo", "auth.sqlite"))
	if err != nil {
		slog.Error("failed to initialize authentication", "error", err)
		os.Exit(1)
	}
	defer authMgr.Close()
	h.Auth = authMgr

	mux := http.NewServeMux()

	staticSub, _ := fs.Sub(staticFS, "static")
	mux.Handle("GET /static/", http.StripPrefix("/static/", http.FileServer(http.FS(staticSub))))

	s.RegisterAPI(mux)
	s.RegisterWebEvents(mux)

	mux.HandleFunc("GET /login", h.Login)
	mux.HandleFunc("POST /login", h.Login)
	mux.HandleFunc("GET /settings/password", h.PasswordSettings)
	mux.HandleFunc("POST /settings/password", h.PasswordSettings)
	mux.HandleFunc("GET /api/items/{id}", h.GetItemByID)
	mux.HandleFunc("POST /api/items", h.CreateItem)
	mux.HandleFunc("PATCH /api/items/{id}", h.UpdateItemByID)
	mux.HandleFunc("POST /api/query/ids", h.QueryIDsByCEL)
	mux.HandleFunc("GET /api/app/bootstrap", h.AppBootstrap)
	mux.HandleFunc("POST /api/app/changes", h.AppChanges)
	mux.HandleFunc("GET /api/app/configs", h.AppConfigs)
	mux.HandleFunc("PUT /api/app/configs/{path}", h.AppConfigUpdate)
	mux.HandleFunc("GET /api/app/sync-clients", h.AppSyncClients)
	mux.HandleFunc("POST /api/app/sync-clients/{clientId}/approve", h.AppSyncClientApprove)
	mux.HandleFunc("POST /api/app/sync-clients/{clientId}/revoke", h.AppSyncClientRevoke)
	mux.HandleFunc("POST /api/app/password", h.AppPassword)
	mux.HandleFunc("GET /api/app/items/{id}/actions", h.AppItemActions)
	mux.HandleFunc("POST /api/app/actions/{actionName}", h.AppAction)
	mux.HandleFunc("GET /api/app/api-keys", h.AppAPIKeys)
	mux.HandleFunc("POST /api/app/api-keys", h.AppAPIKeyCreate)
	mux.HandleFunc("POST /api/app/api-keys/{id}/revoke", h.AppAPIKeyRevoke)

	mux.HandleFunc("POST /webhook/{source}", h.WebhookReceive)
	mux.HandleFunc("GET /ping", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte("pong"))
	})
	mux.HandleFunc("GET /healthz", healthCheck)

	spaSub, _ := fs.Sub(spaFS, "web/dist")
	mux.HandleFunc("GET /{path...}", serveSPA(spaSub))

	handler := requestLoggingMiddleware(h.TimingMiddleware(authMgr.Middleware(h.CSRFMiddleware(h.ConfigReloadMiddleware(mux)))))
	slog.Info("serve listening", "listen", appCfg.Server.Listen, "db_path", appCfg.Server.DBPath, "exo_dir", dir)
	if err := http.ListenAndServe(appCfg.Server.Listen, handler); err != nil {
		slog.Error("serve stopped", "error", err)
		os.Exit(1)
	}
}

func healthCheck(w http.ResponseWriter, _ *http.Request) {
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	_, _ = w.Write([]byte("ok\n"))
}

func serveSPA(spa fs.FS) http.HandlerFunc {
	fileServer := http.FileServer(http.FS(spa))
	return func(w http.ResponseWriter, r *http.Request) {
		path := strings.TrimPrefix(r.URL.Path, "/")
		if path == "" {
			path = "index.html"
		}
		if f, err := spa.Open(path); err == nil {
			_ = f.Close()
			fileServer.ServeHTTP(w, r)
			return
		}
		r2 := r.Clone(r.Context())
		r2.URL.Path = "/index.html"
		fileServer.ServeHTTP(w, r2)
	}
}

func initAuth(dbPath string, legacyPaths ...string) (*auth.Manager, error) {
	mgr, err := auth.New(dbPath)
	if err != nil {
		return nil, err
	}
	for _, path := range legacyPaths {
		if err := mgr.ImportLegacy(path); err != nil {
			_ = mgr.Close()
			return nil, err
		}
	}
	password, err := mgr.EnsurePassword()
	if err != nil {
		_ = mgr.Close()
		return nil, err
	}
	if password != "" {
		slog.Info("initial admin password generated", "password", password)
	}
	return mgr, nil
}

type loggingResponseWriter struct {
	http.ResponseWriter
	status int
	bytes  int
}

func (w *loggingResponseWriter) WriteHeader(status int) {
	if w.status != 0 {
		return
	}
	w.status = status
	w.ResponseWriter.WriteHeader(status)
}

func (w *loggingResponseWriter) Write(b []byte) (int, error) {
	if w.status == 0 {
		w.status = http.StatusOK
	}
	n, err := w.ResponseWriter.Write(b)
	w.bytes += n
	return n, err
}

func (w *loggingResponseWriter) Flush() {
	if flusher, ok := w.ResponseWriter.(http.Flusher); ok {
		flusher.Flush()
	}
}

func requestLoggingMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		lw := &loggingResponseWriter{ResponseWriter: w}
		next.ServeHTTP(lw, r)
		status := lw.status
		if status == 0 {
			status = http.StatusOK
		}
		slog.Info("http request",
			"method", r.Method,
			"path", r.URL.Path,
			"query", r.URL.RawQuery,
			"status", status,
			"bytes", lw.bytes,
			"duration_ms", time.Since(start).Milliseconds(),
			"remote_addr", r.RemoteAddr,
			"user_agent", r.UserAgent(),
		)
	})
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

	// Get the absolute path to the xo binary and directory
	exePath, err := os.Executable()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error determining executable path: %v\n", err)
		os.Exit(1)
	}

	absDir, err := filepath.Abs(dir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error determining absolute directory path: %v\n", err)
		os.Exit(1)
	}

	configPath := filepath.Join(helixDir, "languages.toml")
	configContent := fmt.Sprintf(`[[language]]
name = "markdown"
language-servers = ["exokephalos"]

[language-server.exokephalos]
command = "%s"
args = ["lsp"]

[language-server.exokephalos.environment]
EXO_DIR = "%s"
`, exePath, absDir)

	if err := os.WriteFile(configPath, []byte(configContent), 0644); err != nil {
		fmt.Fprintf(os.Stderr, "Error writing languages.toml: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Created %s\n", configPath)
	fmt.Printf("Helix will now use %s lsp for markdown files in %s\n", exePath, absDir)
}

func runImport(exoDir string) {
	if len(os.Args) < 4 {
		fmt.Fprintf(os.Stderr, "Usage: xo import <source-dir> <type>\n")
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
		fmt.Fprintf(os.Stderr, "Usage: xo export <output-dir> [--type <type>]\n")
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
