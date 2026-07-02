# Agents

## Project Overview

exokephalos is a Go application for personal knowledge management with a TUI (terminal UI), a web interface, and an LSP server. It uses flat-file markdown with YAML frontmatter as its storage layer (no database). The TUI is the default mode; the web interface is launched with `exo serve`; the LSP server is launched with `exo lsp`.

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

- All references to "exo" and "exokephalos" (except for environment variables like `EXO_DIR` and `EXO_PORT`) must be lowercase.
- All data is stored as markdown files with YAML frontmatter in the directory specified by `EXO_DIR`.
- `exo` (no args) launches the TUI; `exo serve` starts the HTTP server on `:8293`; `exo lsp` starts the LSP server on stdio.
- Routes use Go 1.22+ stdlib routing patterns (method + path).
- Templates use Go `html/template`.
- CSS is built with Tailwind v4 (`npm run build:css`).
- No database — the filesystem is the source of truth.
- TUI, web interface, and LSP should have feature parity; all use `internal/repo` for data access.
- The LSP supports wikilinks `[[id]]` for linking between notes, tag completion in frontmatter and body (`:tag:` syntax), hover previews, and go-to-definition.

## ID Generation Scheme

- Generated IDs are fully lowercase and case-insensitive.
- Scheme:
  - Base32 encoded days since 1989-01-17.
  - Base32 random string of 4 characters.
  - Prefixed with zeroes if the resultant string is shorter than 7 characters.

## Views and Subviews (CEL-Based)

- Views are configured via `.toml` files in the `.exo/` directory inside `EXO_DIR` (e.g. `.exo/notes.toml`, `.exo/books.toml`).
- Views filter notes using Google Common Expression Language (CEL) expressions.
- The CEL environment exposes `type` (string), `tags` (list of strings), and `fm` (full map of frontmatter fields).
- Subviews provide tabbed filtering using additional CEL sub-expressions.

## Goodreads & Reading Stats

- Goodreads books are imported directly inside the TUI by opening the action menu (`a` or `Space`), pressing `i`, and pasting the Goodreads book URL.
- To display reading charts via Chart.js on the web interface, set `stats_template = "books/stats"` in the view configuration.

## Custom Actions

- Configured under the `[actions]` section in config files (e.g., `.exo/actions.toml`).
- Each action specifies:
  - `description`: label.
  - `filter`: CEL boolean expression to check applicability.
  - `expr`: `yq` mutation syntax (e.g. `.tags -= ["reading"] | .tags += ["read"]`).

## Import and Export Commands

- `exo import <source-directory> <type>` scans raw files, normalizes frontmatter (generating lowercase IDs, converting dates to unquoted YAML timestamps), and writes to the workspace folder layout: `<EXO_DIR>/<type>/<year>/<month>/<slugified-title>.md`.
- `exo export <output-directory> [--type <type>]` exports repository files, cleaning up application-specific frontmatter fields (`id`, `type`, `created`) and resolving naming conflicts by appending suffixes (`-1.md`).

## Feature Requests

When receiving a feature request, always ask whether it applies to:
- The TUI only
- The web interface only
- Both (preferred — maintain feature parity)

## Development

- Use `task dev:tui` to spin up a sandboxed TUI environment with imported example files.
- Use `task dev:serve` to spin up a sandboxed Web UI environment with imported example files.
- Use `task test` to run all package integration tests.

## Version

- Version is set at build time via `-ldflags` in `internal/version/`.
- The version string is `git describe --tags --always --dirty` output (or `dev` fallback).
- Build time is injected as an RFC 3339 UTC timestamp.
- Displayed via `exo version` / `exo --version` and in the web UI footer.
- Add `-ldflags="{{.LDFLAGS}}"` to any new `go build` commands in `Taskfile.yml`.

## Build

```bash
task build   # Builds CSS + Go binary (exo)
```

## Deploy

> [!IMPORTANT]
> **Consent Directive**: Never run deployment commands (`task deploy` or transfers to remote environments like fedora or imp) without explicit user consent.

```bash
task deploy        # Builds locally + deploys to fedora and imp (requires consent)
task deploy-local  # Builds and installs to ~/.local/bin/
```

## Testing

```bash
task test    # Runs all tests
```
