---
type: doc
tags: []
id: quickstart
created: 2026-07-09
title: "Quickstart"
---

# Quickstart

Install `xo` first, then create a data directory. xo stores all markdown files and configuration in one data directory.

## Install

Download prebuilt binaries from the [GitHub releases page](https://github.com/gnur/exokephalos/releases). CI builds binaries for Linux, macOS, and Windows on both `amd64` and `arm64`.

On Linux or macOS:

```bash
chmod +x xo-*
sudo mv xo-* /usr/local/bin/xo
```

You can also run the web interface with Docker:

```bash
docker run --rm \
  -p 8293:8293 \
  -v "$HOME/notes:/data" \
  ghcr.io/gnur/exokephalos:latest
```

The container runs `xo serve` and uses `/data` as `EXO_DIR`.

## 1. Create a data directory

```bash
mkdir -p ~/notes/.exo
export EXO_DIR=~/notes
```

`EXO_DIR` is the only required environment variable. It points at your exo data directory. If it is not set, exo uses `./example-repo`.

## 2. Add a minimal workspace config file

Create `~/notes/exo.fnl`:

```fennel
{:default-view :notes
 :views {:notes {:name "Notes"
                 :key "n"
                 :show-tags true
                 :when (fn [note] (= note.type "note"))}}
 :actions {}}
```

Workspace config lives in `exo.fnl` and optional `modules/**/*.fnl` or `modules/**/*.lua` files in `EXO_DIR`. The `.exo/` directory is local-only and stores app state such as cache databases, sync keys, `.exo/tui.fnl`, `.exo/serve.fnl`, and `.exo/permissions.fnl`.

Each view requires:

| Field | Purpose |
| --- | --- |
| `name` | Display name in the TUI and web UI |
| `key` | Unique ordering/navigation key for the view |
| `when` | Fennel predicate selecting matching markdown files |

Optional fields include `show_tags`, `title_field`, `subtitle_field`, `sort_field`, `sort_order`, `preview_template`, `stats_template`, and `subviews`.

## 3. Add your first note

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

## 4. Run xo

```bash
EXO_DIR=~/notes xo        # TUI
EXO_DIR=~/notes xo serve  # Web UI on :8293
EXO_DIR=~/notes xo lsp    # LSP server on stdio
```

## Sync and API

`xo serve` runs the regular web UI backed by SQLite sync storage. TUI clients configure `.exo/tui.fnl`, run `start-sync` once, and then continue automatically after approval in the web UI's `sync clients` tab.

The HTTP API, signed TUI sync endpoints, and SSE refresh endpoints are documented in [docs/api.md](../../docs/api.md).
