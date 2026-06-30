# Agents

## Project Overview

Exokephalos is a Go application for personal knowledge management with a TUI (terminal UI), a web interface, and an LSP server. It uses flat-file markdown with YAML frontmatter as its storage layer (no database). The TUI is the default mode; the web interface is launched with `exo serve`; the LSP server is launched with `exo lsp`.

## Architecture

- **Entry point:** `main.go` — subcommand dispatch (`exo` = TUI, `exo serve` = web server, `exo lsp` = LSP server)
- **`internal/tui/`** — Bubbletea-based terminal UI
- **`internal/handlers/`** — HTTP handler functions for the web interface
- **`internal/lsp/`** — Language Server Protocol implementation for editor integration
- **`internal/markdown/`** — Frontmatter parsing and writing utilities
- **`internal/repo/`** — Data access layer (reads/writes markdown files on disk)
- **`templates/`** — Go HTML templates (web interface)
- **`static/`** — Static assets (compiled Tailwind CSS, JS)

## Key Conventions

- All data is stored as markdown files with YAML frontmatter in the directory specified by `EXO_DIR`
- `exo` (no args) launches the TUI; `exo serve` starts the HTTP server on `:8293`; `exo lsp` starts the LSP server on stdio
- Routes use Go 1.22+ stdlib routing patterns (method + path)
- Templates use Go `html/template`
- CSS is built with Tailwind v4 (`npm run build:css`)
- No database — the filesystem is the source of truth
- TUI, web interface, and LSP should have feature parity; all use `internal/repo` for data access
- The LSP supports wikilinks `[[id]]` for linking between notes, tag completion in frontmatter and body (`:tag:` syntax), hover previews, and go-to-definition

## Feature Requests

When receiving a feature request, always ask whether it applies to:
- The TUI only
- The web interface only
- Both (preferred — maintain feature parity)

## Development

- Use `task dev` for hot-reload development (Air for Go, watch for CSS)
- Use `task test` to run integration tests
- The `example-repo/` directory contains sample data for local development

## Build

```bash
task build   # Builds CSS + Go binary (exo)
```

## Deploy

```bash
task deploy        # Builds locally + deploys to fedora and imp
task deploy-local  # Builds and installs to ~/.local/bin/
```

## Testing

```bash
task test    # Runs integration_test.go
```
