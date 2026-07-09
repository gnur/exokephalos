---
type: doc
tags: []
id: viewsdoc
created: 2026-07-09
title: "View Configurations"
---

# View Configurations

Views are custom collections that define how markdown files are queried, displayed, and filtered.

exo reads configuration from `.exo/*.toml` files inside `EXO_DIR`, or from a single `.exo.toml` file at the root of `EXO_DIR` when no `.exo/*.toml` files exist. The `.exo/` directory style is recommended for splitting view and action configuration into separate files.

exo also adds a built-in `All` view with key `0`. This view uses the filter `true`, so it shows every item regardless of type in both the TUI and web interface at `/views/all`.

## Minimal View

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

## Required Fields

| Field | Purpose |
| --- | --- |
| `name` | Display name in navigation menus |
| `key` | Unique ordering/navigation key for the view |
| `filter` | CEL expression evaluated against item frontmatter |
| `template` | Go template used when creating a new item |

## Optional Fields

| Field | Purpose |
| --- | --- |
| `show_tags` | Show tag sidebar and tag badges |
| `title_field` | Frontmatter field used as the list title; defaults to `title` |
| `subtitle_field` | Frontmatter field used as the list subtitle |
| `sort_field` | Frontmatter field used for ordering; defaults to `created` |
| `sort_order` | `asc` or `desc`; defaults to `desc` |
| `preview_template` | TUI preview template; receives frontmatter fields plus `.Body` |
| `stats_template` | Embedded stats template name, such as `books/stats` |

## CEL Filters

View and subview filters use CEL. The environment exposes:

| Variable | Type | Description |
| --- | --- | --- |
| `type` | `string` | The item frontmatter `type` |
| `tags` | `list<string>` | The item frontmatter `tags` |
| `fm` | `map<string, dyn>` | The full frontmatter map |

Examples:

```cel
type == "note"
"todo" in tags && !("done" in tags)
type == "book" && fm["pages"] > 300
```

## Subviews

Subviews are tabbed filters inside a view. They narrow the parent view result.

```toml
[[views.notes.subviews]]
name = "All"
filter = "true"

[[views.notes.subviews]]
name = "Todo"
filter = '"todo" in tags && !("done" in tags)'
```

If a view has no subviews, exo adds an `All` subview with `filter = "true"`.

## Template Variables

These values are filled automatically when creating items:

| Variable | Value |
| --- | --- |
| `{{.Date}}` | Current date as `2006-01-02` |
| `{{.DateTime}}` | Current datetime as `2006-01-02T15:04:05` |
| `{{.Year}}` | Current year |
| `{{.Month}}` | Current month |
| `{{.Day}}` | Current day |
| `{{.ID}}` | Generated lowercase ID |
| `{{.Slug}}` | Slug derived from `{{.Title}}` |

Any other `{{.VarName}}` prompts in the TUI and becomes a form field in the web UI.
