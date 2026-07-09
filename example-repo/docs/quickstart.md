---
type: doc
tags: []
id: quickstart
created: 2026-07-09
title: "Quickstart"
---

# Quickstart

This assumes you already have the `exo` binary available. exo stores all markdown files and configuration in one data directory.

## 1. Create a data directory

```bash
mkdir -p ~/notes/.exo
export EXO_DIR=~/notes
```

`EXO_DIR` is the only required environment variable. It points at your exo data directory. If it is not set, exo uses `./example-repo`.

## 2. Add a minimal config file

Create `~/notes/.exo/notes.toml`:

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

Configuration can live in `.exo/*.toml` files or in a single `.exo.toml` file at the root of `EXO_DIR`. The `.exo/` directory style is recommended because it lets you split views and actions into separate files.

Each view requires:

| Field | Purpose |
| --- | --- |
| `name` | Display name in the TUI and web UI |
| `key` | Unique ordering/navigation key for the view |
| `filter` | CEL expression selecting matching markdown files |
| `template` | Go template used when creating a new item |

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

## 4. Run exo

```bash
EXO_DIR=~/notes exo        # TUI
EXO_DIR=~/notes exo serve  # Web UI on :8293
EXO_DIR=~/notes exo lsp    # LSP server on stdio
```
