# Introduction to exokephalos

exokephalos is a lightweight, local-first personal knowledge management system. In local mode it reads and writes flat-file markdown documents with YAML frontmatter. Optional sync mode adds a central SQLite-backed server while TUI clients keep local markdown files.

It offers three distinct interfaces:
1. **TUI**: A terminal UI built with Bubble Tea for fast, keyboard-driven navigation (`exo`).
2. **Web UI**: A modern, responsive web server powered by HTMX (`exo serve`).
3. **LSP Server**: A Language Server Protocol server that provides auto-completions, hover previews, and wiki-link go-to-definition in text editors (`exo lsp`).

Start with the Quickstart doc to create a data directory, set `EXO_DIR`, and add the first root-level workspace configuration file.

---

## Fully Configurable Types

Unlike typical knowledge bases that enforce a single structured note format, exokephalos is fully customizable. Every item in the repository is typed using the `type` field in its frontmatter:

```yaml
---
id: nkyuw4u
type: book
title: "Project Hail Mary"
author: "Andy Weir"
tags: ["read"]
created: 2026-06-30T15:00:00Z
---
```

Because there is no fixed database schema, you can introduce any arbitrary type (e.g. `book`, `note`, `secret`, `recipe`, `log`, `webhook`) simply by adding files containing that type to your workspace.

---

## Common Expression Language (CEL) Views

Views define the collections displayed in both the TUI and Web interfaces. Views are configured via root-level `.toml` files in `EXO_DIR`, such as `notes.toml` or `books.toml`. The `.exo/` directory is reserved for local app state and machine-local config.

Each view compiles a [Google CEL (Common Expression Language)](https://github.com/google/cel-go) filter expression to select matching markdown files from the repository.

### CEL Variables

The CEL execution environment provides three variables:

| Variable | Type | Description |
| --- | --- | --- |
| `type` | `string` | The value of the `type` frontmatter field. |
| `tags` | `list<string>` | The list of tags attached to the item. |
| `fm` | `map<string, dyn>` | The complete frontmatter key-value map, allowing access to any arbitrary fields. |

### Example Filter Expressions

- **Select by type**:
  `type == "note"`
- **Complex logic**:
  `type == "note" && !("read" in tags || "to-read" in tags || "reading" in tags)`
- **Query arbitrary frontmatter fields via `fm`**:
  `fm.status == "active" && fm.priority > 3`
- **Check list containment**:
  `"todo" in tags && !("done" in tags)`
