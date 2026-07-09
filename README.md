# Exokephalos

A self-hosted, file-based personal knowledge management and life-tracking application. All data is stored as plain markdown files with YAML frontmatter — no database required. Views are fully configurable via a `.exo/` directory containing TOML files (or a single `.exo.toml` file) using CEL filter expressions.

## Features

- **Fully generic, config-driven** — Define any type of content (notes, books, articles, logs, etc.) via configuration files
- **Config-driven views** — Define any number of views with CEL filter expressions on frontmatter
- **Subviews** — Narrow a view with additional filters (shown as tabs)
- **Tag filtering** — Views with `show_tags = true` get a clickable tag sidebar
- **Actions** — User-triggered frontmatter transformations with CEL matching + yq expressions
- **Book tracking** — Manage a reading list with statuses (to-read, reading, read)
- **Webhooks** — Receive and store incoming webhooks as markdown files
- **Stats pages** — Per-view stats with embedded templates
- **Dual interface** — Full-featured TUI (default) and web interface (`exo serve`)
- **LSP server** — Language Server Protocol support for editors (`exo lsp`)

## Configuration

All views are defined in `.exo/` configuration files (or a single `.exo.toml` file) at the root of your data directory (`EXO_DIR`). The app scans all `.md` files recursively and filters them by frontmatter using [CEL expressions](https://github.com/google/cel-go).

### Minimal example

```toml
default_view = "notes"

[views.notes]
name = "Notes"
key = "n"
filter = 'type == "note"'
show_tags = true
title_field = "title"
sort_field = "created"
sort_order = "desc"
path_template = "notes/{{.ID}}-{{.Slug}}.md"
template = """
---
type: note
tags: []
id: {{.ID}}
created: {{.Date}}
title: "{{.Title}}"
---

# {{.Title}}
"""
```

### Full example with subviews and stats

```toml
default_view = "notes"

[views.notes]
name = "Notes"
key = "n"
filter = 'type == "note" && !("read" in tags || "to-read" in tags)'
show_tags = true
title_field = "title"
sort_field = "created"
sort_order = "desc"
path_template = "zettelkasten/{{.ID}}-{{.Slug}}.md"
template = """
---
type: note
tags: []
id: {{.ID}}
created: {{.Date}}
title: "{{.Title}}"
---

# {{.Title}}
"""

[[views.notes.subviews]]
name = "All"
filter = "true"

[[views.notes.subviews]]
name = "Recipes"
filter = '"recept" in tags'

[views.books]
name = "Books"
key = "b"
filter = '("read" in tags || "to-read" in tags || "reading" in tags)'
show_tags = false
title_field = "title"
subtitle_field = "author"
sort_field = "added"
sort_order = "desc"
path_template = "books/{{.Slug}}.md"
stats_template = "books/stats"
template = """
---
type: book
tags: [to-read]
author: {{.Author}}
title: "{{.Title}}"
pages: {{.Pages}}
cover: "{{.Cover}}"
url: "{{.URL}}"
added: {{.Date}}
started:
finished:
---
"""
preview_template = """
# {{.title}}

**Author:** {{.author}}
**Pages:** {{.pages}}
**Added:** {{.added}}
"""

[[views.books.subviews]]
name = "All"
filter = "true"

[[views.books.subviews]]
name = "To Read"
filter = '"to-read" in tags'

[[views.books.subviews]]
name = "Reading"
filter = '"reading" in tags'

[[views.books.subviews]]
name = "Read"
filter = '"read" in tags'

[views.articles]
name = "Articles"
key = "a"
filter = 'type == "article"'
show_tags = true
title_field = "title"
sort_field = "published"
sort_order = "desc"
template = """
---
type: article
title: "{{.Title}}"
author: ""
url: ""
published: {{.Date}}
tags: []
---
"""
```

### Config reference

#### Top-level

| Field | Description |
|-------|-------------|
| `default_view` | View ID to show on startup (TUI) and root redirect (web) |

#### `[views.<id>]`

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Display name |
| `key` | yes | Unique single-char key for TUI view switching |
| `filter` | yes | CEL expression evaluated against frontmatter |
| `show_tags` | no | Show tag sidebar + tag badges on items (default: false) |
| `title_field` | no | Frontmatter field for item title (default: "title") |
| `subtitle_field` | no | Frontmatter field for subtitle line |
| `sort_field` | no | Frontmatter field to sort by (default: "created") |
| `sort_order` | no | "asc" or "desc" (default: "desc") |
| `template` | yes | Go template for file content on creation |
| `preview_template` | no | Go template for TUI preview pane (receives frontmatter + `.Body`) |
| `stats_template` | no | Embedded stats template name (e.g. "books/stats") |

#### `[[views.<id>.subviews]]`

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Tab label |
| `filter` | yes | CEL expression (ANDed with parent view filter) |

#### `[actions.<name>]`

Actions are user-triggered transformations that modify an item's frontmatter on disk. The `filter` field uses CEL to determine which items the action applies to, and `expr` uses [yq](https://github.com/mikefarah/yq) syntax to describe the transformation.

| Field | Required | Description |
|-------|----------|-------------|
| `filter` | yes | CEL expression — item must match for the action to appear |
| `expr` | yes | yq expression describing the frontmatter mutation |
| `description` | yes | Human-readable label shown in the UI |

**Example:**

```toml
[actions.finish-book]
filter = '"reading" in tags'
expr = '.tags -= ["reading"] | .tags += ["read"] | .finished = now'
description = "Mark book as finished reading"

[actions.archive-note]
filter = 'type == "note" && "active" in tags'
expr = '.tags -= ["active"] | .tags += ["archived"] | .archived_at = now'
description = "Archive this note"
```

**yq expression reference:**

| Operation | Expression |
|-----------|-----------|
| Remove tag, add another | `.tags -= ["old"] \| .tags += ["new"]` |
| Add current datetime | `.updated = now` |
| Add formatted date | `.created = (now \| format_datetime("2006-01-02"))` |
| Remove a field | `del(.deprecated)` |

### CEL filter variables

| Variable | Type | Description |
|----------|------|-------------|
| `type` | string | Value of frontmatter `type` field |
| `tags` | list(string) | Value of frontmatter `tags` field |
| `fm` | map(string, dyn) | Full frontmatter map for advanced queries |

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

Run `exo` to launch the terminal UI. Keybindings:

| Key | Action |
|-----|--------|
| `g` | Open view selector popup |
| `tab` | Cycle subviews within current view |
| `j/k` | Navigate list |
| `h/l` | Switch pane (tags/list/preview) |
| `space` | Toggle tag selection (tags pane) |
| `/` | Search (filters by title) |
| `esc` | Clear search / close popup |
| `a` | Open actions popup — press the first letter of an action name to execute (e.g. `f` for finish-book, `i` for Goodreads import) |
| `q` | Quit |

### Web Interface

Run `exo serve` to start the HTTP server on port 8293.

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
| `GET /api/items/{id}` | Return an item as JSON with `frontmatter` and `body` |
| `PATCH /api/items/{id}` | Replace an item's `frontmatter` and/or `body` |
| `POST /api/query/ids` | Return sorted item IDs matching a CEL expression |
| `POST /webhook/{source}` | Receive webhook |
| `/books/import` | Import book from Goodreads URL |

API endpoints return JSON. Error responses use `{"error":"..."}`.

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

`POST /api/query/ids` accepts a plain CEL expression in the request body using the same environment as views: `type`, `tags`, and `fm`.

```bash
curl -s http://localhost:8293/api/query/ids \
  -H 'Content-Type: text/plain' \
  --data-binary 'type == "book" && "reading" in tags'
```

```json
{"ids":["apibook"]}
```

### LSP Server

Run `exo lsp` to start the Language Server Protocol server over stdio. This provides IDE-like features for editing notes in any LSP-compatible editor.

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
    cmd = { 'exo', 'lsp' },
    filetypes = { 'markdown' },
    root_dir = function(fname)
      return vim.fs.dirname(vim.fs.find({ '.exo.toml', '.exo' }, { upward = true })[1])
    end,
    settings = {},
  },
}
require('lspconfig').exokephalos.setup{}
```

VS Code: Create an extension that runs `exo lsp` as the language server (see [LSP client examples](https://code.visualstudio.com/api/language-extensions/language-server-extension-guide)).

Helix: Run `exo helix-init` to automatically create `.helix/languages.toml` with the LSP configuration.

## Tech Stack

- Go (stdlib `net/http` with 1.22+ routing)
- [CEL-Go](https://github.com/google/cel-go) for filter expressions
- [yq](https://github.com/mikefarah/yq) for action expressions
- [Bubbletea](https://github.com/charmbracelet/bubbletea) + [Lipgloss](https://github.com/charmbracelet/lipgloss) + [Glamour](https://github.com/charmbracelet/glamour) for TUI
- Tailwind CSS v4 (web interface)
- Go `html/template` for web rendering
- Flat-file markdown storage (git-friendly)

## Getting Started

### Prerequisites

- Go 1.26+
- Node.js (for Tailwind CSS)
- [Task](https://taskfile.dev) (optional, for convenience commands)

### Running

```bash
# Install CSS dependencies
npm install

# Build
task build

# Run TUI (default)
EXO_DIR=/path/to/your/notes ./exo

# Run web server
EXO_DIR=/path/to/your/notes ./exo serve

### CLI Subcommands

Exokephalos supports several command line subcommands for maintenance and data integration:

#### `import`
Recursively imports all markdown files from a source directory, generates safe 7-character lowercase base32 IDs for them, and writes them into the structured `EXO_DIR` directory.
```bash
EXO_DIR=/path/to/your/notes ./exo import <source-dir> <type>
```

#### `export`
Exports all items from your Exokephalos repository to a specified output folder, organizing them by type and date.
* Path structure: `<output-dir>/<type>/<year>/<month>/<slug-title>.md`
* Cleans output files by removing exo-specific metadata fields (`id`, `type`, and `created` from frontmatter).
* Automatically resolves filename conflicts by appending sequential suffixes (`-1`, `-2`).
```bash
# Export all items
EXO_DIR=/path/to/your/notes ./exo export /path/to/output/dir

# Export a specific item type only
EXO_DIR=/path/to/your/notes ./exo export /path/to/output/dir --type book
```
```

### Development

```bash
task dev  # Hot-reload with Air + CSS watch
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `EXO_DIR` | `./example-repo` | Path to the data directory (must contain `.exo/` or `.exo.toml`) |
| `EDITOR` | `vim` | Editor used by the TUI for editing |

## Data Structure

The data directory is flat — all `.md` files are scanned recursively and filtered purely by frontmatter content. Directory structure is for human organization only.

```
data-dir/
├── .exo/               # Configuration directory
│   ├── cache/          # Cache database directory
│   ├── notes.toml      # Configured view files
│   └── actions.toml
├── notes/              # Notes (type: note)
├── books/              # Books (tagged read/to-read/reading)
├── articles/           # Articles (type: article)
└── webhooks/           # Stored webhooks (type: webhook)
```

Each file uses YAML frontmatter for metadata. An `id` field is auto-injected on creation if not present in the template. You can define any directory structure and content type via the configuration files.

## Deploy

```bash
task deploy        # Build locally + deploy to remote servers
task deploy-local  # Build and install to ~/.local/bin/
```

## Testing

```bash
task test
```
