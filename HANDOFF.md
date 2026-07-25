# Development handoff

Last updated: 2026-07-22

This document is the starting point for a future development session. Read it
before changing the working tree, then use `rust-rewrite-stages.md` for the
larger roadmap and `README.md` for the current operator-facing workflow.

## First actions in a new session

1. Run `git status --short` and preserve all existing changes. The working tree
   is intentionally dirty and the changes listed below have not been committed.
2. Read this file, `README.md`, and the Stage 6 section of
   `rust-rewrite-stages.md`.
3. Run the focused TUI test suite before making another TUI change:

   ```console
   cargo test -p xo --bin xo
   ```

   The Iroh-backed tests bind local sockets and may require sandbox/network
   permission.
4. After changes, run:

   ```console
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   git diff --check
   ```

## Repository and working-tree state

The current branch is based on commit:

```text
97678b0 first steps of tui :)
```

Current uncommitted files at handoff time:

```text
M README.md
M crates/xo-core/src/projection.rs
M crates/xo-core/src/steel_runtime.rs
M crates/xo-core/src/watcher.rs
M crates/xo-core/src/workspace_projection.rs
M crates/xo/src/app.rs
M crates/xo/src/main.rs
M crates/xo/src/session.rs
M rust-rewrite-stages.md
?? HANDOFF.md
```

Do not reset, restore, or overwrite these files. They contain the complete set
of recent configuration and TUI changes.

## Current product behavior

### Command configuration

- Running `xo` without a subcommand opens the TUI.
- Command configuration is read from `~/.config/xo/config.scm`.
- `xo config-init` writes a native Steel configuration document to stdout.
- Command-line flags override the config file.
- The persistent config contains `state-dir`, optional `workspace`, and
  `projection`. A ticket is deliberately not stored there.
- When the config file is absent, xo prints the exact initialization command.
- A new local state directory creates a workspace automatically and records it
  in `STATE_DIR/active-workspace`.

Default command config:

```scheme
(xo-config
  (schema 1)
  (state-dir "~/.local/share/xo")
  (workspace #f)
  (projection "~/notes"))
```

### Workspace configuration

- `xo.scm` is the preferred projected workspace configuration filename.
- Legacy `exo.scm` remains accepted by projection, watcher, runtime, and
  workspace-projection code.
- If the loaded workspace behavior has no views, `WorkspaceSession::behavior`
  creates and replicates `xo.scm` with:
  - `notes`: `type == "note"`, newest first
  - `all`: no predicate, newest first
- The generated workspace file still uses the portable
  `(workspace-config "<descriptor-json>")` envelope. The command-level
  `~/.config/xo/config.scm` is native Steel, but the workspace behavior format
  has not yet been converted to native field forms. This distinction is likely
  to matter in future UX work.

### TUI layout and controls

The screen currently contains:

- a four-line header;
- connection/workspace state in the first two header lines;
- key hints arranged in three columns in the next two header lines;
- a left tag pane;
- a middle filtered-note pane;
- a right raw-Markdown preview pane.

Important controls:

- `Tab`/`Shift-Tab`: change pane
- arrows or `j`/`k`: move through the focused list
- `Space` or `Enter` in the tag pane: toggle the highlighted tag filter
- `/`: open the title-filter input between header and content
- `:`: open searchable view/subview selection
- `c`: prompt for a title, construct default frontmatter, then open `$EDITOR`
- `Enter` or `e` on a note: edit it
- `d`/`u`: delete/restore
- `a`: fuzzy action picker
- `x`, `v`, `y`: conflicts, devices, and synchronization details
- `r`: refresh/retry synchronization
- `q`: quit

Selection is clamped to the currently visible note list. This fixes the earlier
failure where filtering left an invalid index and preview/edit appeared broken.

### Preview

The preview is the canonical serialized Markdown document, including:

```text
---
<YAML frontmatter>
---
<Markdown body>
```

Ratatui spans provide lightweight highlighting for YAML keys and values,
frontmatter delimiters, headings, fenced code, blockquotes, Markdown links, and
wikilinks. Configured preview templates are intentionally ignored in this pane
because the latest product direction is to show the raw note.

Encrypted notes show encrypted raw content until unlocked. After unlocking,
the same full-document preview is rendered with the decrypted body.

### Creation and editing

Pressing `c` now follows this sequence:

1. Show a title input row.
2. Require a non-empty title.
3. Create a draft with `id`, `created`, `tags`, `title`, and `type`.
4. Restore the normal terminal and open the complete Markdown document in
   `$EDITOR`.
5. Parse the edited document and enforce the required fields again.
6. Save the new immutable revision and return to the TUI.

Creation currently uses default frontmatter directly and no longer
automatically applies the first configured template. Template behavior remains
implemented in `xo-core`, but a future TUI template-selection workflow will be
needed if templates should participate in creation again.

External editors may save atomically by replacing their input path. xo reads
the temporary path after the editor exits rather than using
`NamedTempFile::reopen`, which avoids this former macOS error:

```text
original tempfile has been replaced at path ...
```

There is a regression test that replaces the temporary file with `mv`.

## Synchronization server workflow

`README.md` documents the implemented setup:

1. Seed a server workspace with `xo-admin import-workspace`.
2. Start `xo-syncd` against that state directory.
3. Initialize the client config with `xo config-init`.
4. Join using `xo --ticket '<WRITABLE_TICKET>'`.

The README also includes systemd, health/status/metrics, additional clients,
backups, and the workaround for attaching a server to a TUI-created workspace.

Important operational constraint: never open the same state directory
concurrently with `xo`, `xo-admin`, and `xo-syncd`.

## Known limitations and likely next work

### 1. Live synchronization does not automatically resume

This is the most concrete near-term issue.

Iroh Docs persists known useful peers, but reopening a document does not start
its live-sync task. The pinned Iroh 0.98 API requires:

```rust
doc.start_sync(vec![]).await?;
```

The empty list tells Iroh to use its persisted peer list. Currently:

- `WorkspaceSession::open` calls `start_sync` only when a ticket is supplied;
- a later plain `xo` launch reopens local state but does not resume live sync;
- `xo-syncd` lists stored workspace IDs but does not explicitly start sync for
  each stored workspace.

The README truthfully tells users to pass the server ticket again after a TUI
restart. A proper fix should add a ticket-free resume method to
`IrohWorkspace`, call it when reopening the active/configured workspace, and
have `xo-syncd` resume every stored workspace on startup. Add a restart test
before removing the README limitation.

### 2. No headless ticket import for the server

`xo-admin` can create invitations but cannot import a ticket into a server state
directory. Attaching `xo-syncd` to a workspace originally created by the TUI
currently requires launching `xo` once against the server state. A dedicated
`xo-admin import-ticket` command would simplify this workflow.

### 3. Workspace `xo.scm` is still a JSON descriptor envelope

The user explicitly wanted native Steel for the command config and that has
been implemented. Workspace behavior still serializes JSON into a Steel string.
If native editable workspace configuration becomes the next priority, extend
the sandboxed Steel parser with declarative native forms while retaining legacy
descriptor compatibility and the existing ambient-capability tests.

### 4. Template creation UX needs a decision

The core supports templates, but `c` now follows the requested default
frontmatter/title/editor flow. If templates return to the TUI, add an explicit
template picker rather than silently choosing the first template.

### 5. Remaining roadmap work

The main incomplete stages are already checkable in
`rust-rewrite-stages.md`. In particular:

- Stage 0: relay-only validation
- Stage 1: storage-independent services and Go/Rust encryption fixtures
- Stage 7: content workflows and parity
- Stage 8: LSP implementation
- Stage 9: iOS application
- Stage 10: conditional PWA spike

## Important files

- `crates/xo/src/main.rs` — CLI, TUI event loop, creation/editor lifecycle
- `crates/xo/src/app.rs` — TUI model, filtering, tags, view picker, preview,
  highlighting, rendering, and most TUI tests
- `crates/xo/src/config.rs` — native command configuration
- `crates/xo/src/session.rs` — local workspace selection, projection hydration,
  behavior bootstrap, persistence, and Iroh-backed TUI tests
- `crates/xo-core/src/iroh_node.rs` — persistent endpoint and workspace sync API
- `crates/xo-core/src/steel_runtime.rs` — sandboxed Steel and config encoding
- `crates/xo-core/src/projection.rs` — projection validation/materialization
- `crates/xo-core/src/workspace_projection.rs` — bidirectional workspace/projected
  file handling
- `crates/xo-core/src/watcher.rs` — supported projected path watching
- `crates/xo-syncd/src/main.rs` — daemon startup and persistent Iroh peer
- `crates/xo-syncd/src/operator.rs` — health, status, and metrics HTTP API
- `crates/xo-admin/src/main.rs` — import, invitations, backups, diagnostics,
  retirement, and namespace rotation
- `README.md` — user/operator setup guide
- `rust-rewrite-stages.md` — implementation checklist and test gates
- `exo-rs-plan.md` — original rewrite plan
- `oldcodebase/` — legacy implementation and compatibility source

## Last verified state

After the latest raw-preview and title-first creation changes:

```text
cargo test -p xo --bin xo
19 passed; 0 failed

cargo clippy --workspace --all-targets -- -D warnings
passed

cargo fmt --all
passed

git diff --check
passed
```

The complete `cargo test --workspace` suite passed earlier in this same line of
work, before the final preview/title changes. Run it again at the start or end
of the next substantial change; its multi-peer Iroh tests take roughly two
minutes and require local socket access.

## Handoff hygiene

- No tickets, operator tokens, endpoint keys, or other secrets are recorded in
  this document.
- Do not use destructive Git commands on the dirty working tree.
- Prefer focused tests while iterating, then run the full workspace suite.
- Keep README limitations aligned with actual behavior; especially remove the
  repeated-ticket warning only after restart/resume synchronization is tested.
