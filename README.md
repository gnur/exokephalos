# exokephalos

** outside of head, from the greek εγκέφαλος which literally means within the head, or brain **

`xo` (pronounced exo) is a offline first note / personal knowledge / zettelkasten application. It's a TUI for quickly creating, updating and searching notes. It uses markdown with frontmatter for managing metadata. The tui can sync with a sync server that doubles as a PWA host. Everything is offline first.
Views and subviews are configured in Fennel (`exo.fnl` and optional workspace modules) with Fennel predicates.
The sync server also has an API that can be used for querying, retrieving and updating items in the `xo` database.


## Features

- **Fully generic, config-driven** — Define any type of content (notes, books, articles, logs, etc.) via configuration files
- **Config-driven views** — Define any number of views with Fennel predicates over complete notes
- **Subviews** — Narrow a view with additional filters (shown as tabs)
- **Tag filtering** — Views with `show_tags = true` get a clickable tag sidebar
- **Actions** — User-triggered Fennel/Lua note transformations with local capability grants
- **Book tracking** — Manage a reading list with statuses (to-read, reading, read)
- **Webhooks** — Receive and store incoming webhooks as markdown files
- **Stats pages** — Per-view stats with embedded templates
- **Dual interface** — Full-featured TUI (default) and web interface (`xo serve`)
- **LSP server** — Language Server Protocol support for editors (`xo lsp`)
- **Optional sync** — Regular web UI with SQLite-backed central storage, signed TUI clients, approval flow, SSE updates, and local outbox retry

## Quickstart

Install `xo` first, then create a data directory. xo stores all data in a directory you choose and reads configuration from that directory.

### Install

Download prebuilt binaries from the [GitHub releases page](https://github.com/gnur/exokephalos/releases). CI builds artifacts for:

- `linux/amd64` and `linux/arm64`
- `darwin/amd64` and `darwin/arm64`
- `windows/amd64` and `windows/arm64`

On Linux or macOS, make the downloaded binary executable and put it on your `PATH`:

```bash
chmod +x xo-*
sudo mv xo-* /usr/local/bin/xo
```

You can also run the web interface as a container:

```bash
docker run --rm \
  -p 8293:8293 \
  -v "$HOME/notes:/data" \
  ghcr.io/gnur/exokephalos:latest
```

The container runs `xo serve`, exposes port `8293`, and uses `/data` as `EXO_DIR`.

### 1. Create a data directory

```bash
mkdir -p ~/notes/.exo
export EXO_DIR=~/notes
```

`EXO_DIR` is the only required environment variable. It points at your exo data directory. If it is not set, exo uses `./example-repo`.

### 2. Add a minimal config file

Create `~/notes/exo.fnl`:

```fennel
{:default-view :notes
 :views {:notes {:name "Notes" :key "n" :show-tags true
                 :when (fn [note] (= note.type "note"))
                 }}
 :actions {}}
```

Workspace config lives in `exo.fnl` and optional `modules/**/*.fnl` or `modules/**/*.lua` files in `EXO_DIR`. The `.exo/` directory is local-only and holds application settings, permissions, cache databases, and sync keys.

Each view needs:

| Field | Purpose |
|-------|---------|
| `name` | Display name in the TUI and web UI |
| `key` | Unique ordering/navigation key for the view |
| `when` | Fennel predicate selecting matching markdown files |

Optional fields such as `show_tags`, `title_field`, `subtitle_field`, `sort_field`, `sort_order`, `preview_template`, `stats_template`, and `subviews` control display and filtering behavior.

### 3. Add your first note

You can create notes from the TUI, or add a markdown file manually:

```bash
mkdir -p ~/notes/notes
cat > ~/notes/notes/first-note.md <<'EOF'
---
type: note
tags: []
id: first01
created: 2026-07-09
title: "First note"
---

# First note

Hello from exo.
EOF
```

Every item is a markdown file with YAML frontmatter. The `type` field determines which view can see it; the example view above shows items where `type == "note"`.

### 4. Run xo

```bash
EXO_DIR=~/notes xo        # TUI
EXO_DIR=~/notes xo serve  # Web UI on :8293
EXO_DIR=~/notes xo lsp    # LSP server on stdio
```

## Configuration

Views and actions are defined in `exo.fnl`, with optional Fennel or Lua modules below `modules/` in your data directory (`EXO_DIR`). The app scans all `.md` files recursively and filters them with Fennel predicates.

The `.exo/` directory is local-only. It is used for `.exo/tui.fnl`, `.exo/serve.fnl`, `.exo/permissions.fnl`, `.exo/cache.sqlite`, generated sync keys, and server databases. These files are not workspace config and are never synced.

`exo.fnl` is required. Modules resolve only from `modules/`, preferring `.fnl` over `.lua` for the same module name.

### Minimal example

```fennel
{:default-view :notes
 :views {:notes {:name "Notes" :key "n" :show-tags true
                 :when (fn [note] (= note.type "note"))
                 }}
 :actions {}}
```

### Full example with subviews and stats

```fennel
{:default-view :notes
 :views {:notes {:name "Notes" :key "n" :show-tags true
                 :when (fn [note] (= note.type "note"))
                 :subviews [{:name "All" :when (fn [_] true)}
                            {:name "Recipes" :when (fn [note] (has-tag note.tags "recept"))}]}
         :books {:name "Books" :key "b" :show-tags false :title-field "title"
                 :subtitle-field "author" :sort-field "added" :stats-template "books/stats"
                 :when (fn [note] (= note.type "book"))
                 }}
 :actions {}}
```

Workspace code provides `has-tag`, `add-tag`, `remove-tag`, and `now`. The tag helpers return a new list; `now` returns an RFC 3339 UTC timestamp. Lua modules use `has_tag`, `add_tag`, and `remove_tag`.

### Config reference

#### Top-level

| Field | Description |
|-------|-------------|
| `:default-view` | View ID to show on startup (TUI) and root redirect (web) |

#### `:views`

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Display name |
| `key` | yes | Unique single-char key for TUI view switching |
| `when` | yes | Fennel predicate evaluated against the complete note |
| `show_tags` | no | Show tag sidebar + tag badges on items (default: false) |
| `title_field` | no | Frontmatter field for item title (default: "title") |
| `subtitle_field` | no | Frontmatter field for subtitle line |
| `sort_field` | no | Frontmatter field to sort by (default: "created") |
| `sort_order` | no | "asc" or "desc" (default: "desc") |
| `preview_template` | no | Go template for TUI preview pane (receives frontmatter + `.Body`) |
| `stats_template` | no | Embedded stats template name (e.g. "books/stats") |

#### Subviews and actions

Subviews use `:name` and a boolean Fennel `:when` predicate. Actions have `:description`, optional `:when`, optional capability `:permissions`, and required `:run`; `:run` receives and returns the complete note.

```fennel
{:actions
 {:mark-done {:description "Mark item as done"
              :when (fn [note] (= note.frontmatter.status "todo"))
              :run (fn [note]
                     (assoc note :frontmatter
                            (assoc note.frontmatter :status "done")))}}}
```

Workspace views/actions no longer use CEL or yq. CEL remains only for API-key authorization and `POST /api/query/ids`.

### Template auto-fill variables

These variables are automatically filled when creating items (never prompt the user):

| Variable | Value |
|----------|-------|
| `{{.Date}}` | `2006-01-02` |
| `{{.DateTime}}` | `2006-01-02T15:04:05` |
| `{{.Year}}` | `2006` |
| `{{.Month}}` | `01` |
| `{{.Day}}` | `02` |
| `{{.ID}}` | Random 7-character lowercase base32 ID |
| `{{.Slug}}` | Derived from `{{.Title}}` (lowercase, dashes) |

Any other `{{.VarName}}` in the template prompts the user (TUI) or shows a form field (web).

## Interfaces

### TUI (default)

Run `xo` to launch the terminal UI. Keybindings:

| Key | Action |
|-----|--------|
| `g` | Open view selector popup |
| `tab` | Cycle subviews within current view |
| `j/k` | Navigate list |
| `h/l` | Switch pane (tags/list/preview) |
| `space` | Toggle tag selection (tags pane) |
| `/` | Search (filters by title) |
| `esc` | Clear search / close popup |
| `:` | Open fuzzy action picker |
| `q` | Quit |

The action picker includes configured actions plus built-ins like Goodreads import, Hardcover search, URL-to-note import, and sync actions when sync is configured. Actions whose CEL filter does not match are grayed out; selecting one shows the required CEL expression.

An `All` view is always available with key `0`; it shows every item regardless of type.

### Web Interface

Run `xo serve` to start the HTTP server on port 8293.

On first boot, the web UI generates a 20-character base32 password, prints it to stdout, and stores only an Argon2id hash in the auth database. The login page has a single password field and an optional `trust this device` checkbox for a long-lived cookie. Change the password from the `password` nav item after logging in.

Routes:

| Route | Description |
|-------|-------------|
| `/views/{viewId}` | List items (supports `?tags=a,b` and `?subview=Name`) |
| `/views/{viewId}/stats` | Stats page (if view has `stats_template`) |
| `/views/{viewId}/new` | Create new item |
| `/views/{viewId}/{itemId}` | View item detail |
| `/views/{viewId}/edit/{itemId}` | Edit raw markdown |
| `/views/{viewId}/delete/{itemId}` | Delete item (POST) |
| `POST /views/{viewId}/items/{itemId}/actions/{name}` | Execute an action on an item |
| `/import-url` | Import a web page as a note |
| `POST /api/items` | Create a note from a URL |
| `GET /api/items/{id}` | Return an item as JSON with `frontmatter` and `body` |
| `PATCH /api/items/{id}` | Replace an item's `frontmatter` and/or `body` |
| `POST /api/query/ids` | Return sorted item IDs matching a CEL expression |
| `GET /api/events` | Browser SSE stream for web UI refreshes |
| `POST /webhook/{source}` | Receive webhook |

API endpoints return JSON. Error responses use `{"error":"..."}`.
See [docs/api.md](docs/api.md) for the complete web, sync, SSE, and request-signing API reference.

`POST /api/items` creates a note from a URL:

```bash
curl -s -X POST http://localhost:8293/api/items \
  -H 'Content-Type: application/json' \
  -d '{"url":"https://example.com/article"}'
```

`GET /api/items/{id}` returns:

```json
{
  "frontmatter": {
    "id": "apibook",
    "type": "book",
    "title": "API Book"
  },
  "body": "Book body\n"
}
```

`PATCH /api/items/{id}` accepts `frontmatter`, `body`, or both. Provided fields replace the complete stored value; omitted fields are preserved.
API keys may read and update items that match their CEL filter; an update must also leave the item matching that filter.

```bash
curl -s -X PATCH http://localhost:8293/api/items/apibook \
  -H 'Content-Type: application/json' \
  -d '{"frontmatter":{"id":"apibook","type":"book","title":"API Book"},"body":"Book body\n"}'
```

The response is the updated item:

```json
{
  "frontmatter": {
    "id": "apibook",
    "type": "book",
    "title": "API Book"
  },
  "body": "Book body\n"
}
```

`POST /api/query/ids` accepts a plain CEL expression in the request body using the same environment as views: `type`, `tags`, and `fm`. API-key requests return only IDs that match both this expression and the key's CEL filter.

```bash
curl -s http://localhost:8293/api/query/ids \
  -H 'Content-Type: text/plain' \
  --data-binary 'type == "book" && "reading" in tags'
```

```json
{"ids":["apibook"]}
```

## Sync Server and Web UI

`xo serve` is always SQLite-backed and always exposes the sync server. The TUI and LSP remain local-first and read/write markdown files under `EXO_DIR`; the web UI reads/writes the server database.

In serve mode:

- `xo serve` stores notes and synced root-level workspace config in SQLite.
- The server does not read or write markdown files.
- TUI clients keep local markdown files and a local `.exo/cache.sqlite` cache/outbox.
- Clients connect with signed requests using a generated ed25519 keypair.
- New clients must be approved in the web UI before they can sync.
- The web UI listens for server-sent revision events and refreshes when notes/config change.

### Start Serve Mode

Optionally create `.exo/serve.fnl` in the server data directory to override the default database path or listen address:

```fennel
{:server {:db-path ".exo/server.sqlite" :listen ":8293"}}
```

Start the server:

```bash
EXO_DIR=/path/to/server-data xo serve
```

With Docker, mount the server data directory at `/data`:

```bash
docker run --rm \
  -p 8293:8293 \
  -v "/path/to/server-data:/data" \
  ghcr.io/gnur/exokephalos:latest
```

Approve clients at:

```text
http://localhost:8293/admin/sync/clients
```

### Configure A TUI Client

Create `.exo/tui.fnl` in the client workspace:

```fennel
{:sync {:server-url "http://localhost:8293" :client-id "laptop"}}
```

Then start the TUI:

```bash
EXO_DIR=/path/to/client-notes xo
```

Open the action picker with `:` and run `start-sync`. The client generates a local ed25519 keypair, sends its public key to the server, and waits for approval. After you approve it in the web UI, the TUI continues automatically; a second `start-sync` is not needed. It uploads local markdown files and root-level workspace config, then keeps reconciling every 5 seconds while approved. The sync status appears in the TUI footer.

The TUI also provides a `sync-outbox` action to inspect pending, failed, and synced operations. The outbox view supports scrolling, status filtering, entry details, retrying one entry, and retrying all failed entries. If the server is offline, local edits continue writing to markdown files and are retried from the outbox when the server is reachable again.

### Run Serve Mode On Kubernetes

Plain Kubernetes manifests are available in `deploy/kubernetes/`. They run exo as a single-replica `StatefulSet` with a `ReadWriteOnce` PVC mounted at `/data`. The web UI stores synced data in SQLite at `/data/.exo/server.sqlite`.

Apply the manifests:

```bash
kubectl apply -k deploy/kubernetes/
```

Wait for the StatefulSet to become ready:

```bash
kubectl rollout status statefulset/exo -n exo
kubectl get pvc -n exo
```

The default service is `ClusterIP`. For local access, port-forward it:

```bash
kubectl port-forward -n exo svc/exo 8293:8293
```

Then open the client approval page:

```text
http://localhost:8293/admin/sync/clients
```

Configure a local TUI client with the forwarded URL:

```fennel
{:sync {:server-url "http://localhost:8293" :client-id "laptop"}}
```

Inside the cluster, the service URL is:

```text
http://exo.exo.svc.cluster.local:8293
```

The manifest is intentionally single-replica because SQLite should not be written by multiple pods at once. Add your own Ingress, TLS, authentication proxy, and backups according to your cluster conventions.

### LSP Server

Run `xo lsp` to start the Language Server Protocol server over stdio. This provides IDE-like features for editing notes in any LSP-compatible editor.

**Features:**
- **Tag completion** — Complete tags in frontmatter (`tags: [...]` or `tags:\n  - ...`) and body (`:tag:` syntax)
- **Link completion** — Complete note links inside `[[...]]` wikilinks (matches by ID or title)
- **Go to definition** — Jump to the target note of a `[[wikilink]]`
- **Hover information** — Preview note content when hovering over a `[[wikilink]]`

**Editor configuration:**

Neovim (using nvim-lspconfig):
```lua
require('lspconfig.configs').exokephalos = {
  default_config = {
    cmd = { 'xo', 'lsp' },
    filetypes = { 'markdown' },
    root_dir = function(fname)
      return vim.fs.dirname(vim.fs.find({ 'exo.fnl', '.exo' }, { upward = true })[1])
    end,
    settings = {},
  },
}
require('lspconfig').exokephalos.setup{}
```

VS Code: Create an extension that runs `xo lsp` as the language server (see [LSP client examples](https://code.visualstudio.com/api/language-extensions/language-server-extension-guide)).

Helix: Run `xo helix-init` to automatically create `.helix/languages.toml` with the LSP configuration.

## Tech Stack

- Go (stdlib `net/http` with 1.22+ routing)
- [CEL-Go](https://github.com/google/cel-go) for filter expressions
- [Fennel](https://fennel-lang.org/) for workspace configuration and actions
- [Bubbletea](https://github.com/charmbracelet/bubbletea) + [Lipgloss](https://github.com/charmbracelet/lipgloss) + [Glamour](https://github.com/charmbracelet/glamour) for TUI
- Tailwind CSS v4 (web interface)
- Go `html/template` for web rendering
- Flat-file markdown storage for TUI/LSP clients; SQLite-backed storage for `xo serve`

## Build And Run

### Prerequisites

- Go 1.26+
- Node.js (for Tailwind CSS)
- [Task](https://taskfile.dev) (optional, for convenience commands)

See [Quickstart](#quickstart) for the required `EXO_DIR` setup and minimal config.

```bash
# Install CSS dependencies
npm install

# Build
task build

# Run TUI (default)
EXO_DIR=/path/to/your/notes ./xo

# Run web server
EXO_DIR=/path/to/your/notes ./xo serve
```

### Run With Docker

The published container image is available from GitHub Container Registry:

```bash
docker pull ghcr.io/gnur/exokephalos:latest
```

Run the web UI with your exo data directory mounted at `/data`:

```bash
docker run --rm \
  -p 8293:8293 \
  -v "/path/to/your/notes:/data" \
  ghcr.io/gnur/exokephalos:latest
```

The image runs `xo serve` by default. To run another subcommand, override the command:

```bash
docker run --rm \
  -v "/path/to/your/notes:/data" \
  ghcr.io/gnur/exokephalos:latest \
  version
```

### CLI Subcommands

Exokephalos supports several command line subcommands for maintenance and data integration:

#### `import`
Recursively imports all markdown files from a source directory, generates safe 7-character lowercase base32 IDs for them, and writes them into the structured `EXO_DIR` directory.
```bash
EXO_DIR=/path/to/your/notes ./xo import <source-dir> <type>
```

#### `export`
Exports all items from your Exokephalos repository to a specified output folder, organizing them by type and date.
* Path structure: `<output-dir>/<type>/<year>/<month>/<slug-title>.md`
* Cleans output files by removing exo-specific metadata fields (`id`, `type`, and `created` from frontmatter).
* Automatically resolves filename conflicts by appending sequential suffixes (`-1`, `-2`).
```bash
# Export all items
EXO_DIR=/path/to/your/notes ./xo export /path/to/output/dir

# Export a specific item type only
EXO_DIR=/path/to/your/notes ./xo export /path/to/output/dir --type book
```

### Development

```bash
task dev  # Hot-reload with Air + CSS watch
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `EXO_DIR` | `./example-repo` | Path to the data directory (normally contains `exo.fnl`, optional modules, and markdown files) |
| `EDITOR` | `vim` | Editor used by the TUI for editing |

## Data Structure

The data directory is flat — all `.md` files are scanned recursively and filtered purely by frontmatter content. Directory structure is for human organization only.

```
data-dir/
├── .exo/               # Local-only app state and machine config
│   ├── cache.sqlite    # TUI cache and sync outbox
│   ├── tui.fnl         # Optional TUI-local sync settings
│   ├── serve.fnl       # Optional server-local settings
│   ├── permissions.fnl # Local action capability grants
│   └── keys/           # Generated client signing keys
├── exo.fnl             # Synced workspace configuration entrypoint
├── modules/            # Optional synced Fennel/Lua workspace modules
├── notes/              # Notes (type: note)
├── books/              # Books (tagged read/to-read/reading)
├── articles/           # Articles (type: article)
└── webhooks/           # Stored webhooks (type: webhook)
```

Each markdown file uses YAML frontmatter for metadata. An `id` field is auto-injected on creation if not present in the template. You can define any directory structure and content type via the root-level workspace configuration files.

## Deploy

```bash
task deploy        # Push the container image and install locally
task deploy-local  # Build and install to ~/.local/bin/
```

## Testing

```bash
task test
```
