# Development handoff

Last updated: 2026-07-26

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
   cargo test -p xo
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
cb74ed2 more work in progress
```

Current uncommitted files at handoff time:

```text
M HANDOFF.md
M README.md
M Cargo.lock
M crates/xo/Cargo.toml
M crates/xo-syncd/Cargo.toml
M crates/xo/src/app.rs
M crates/xo/src/main.rs
M crates/xo/src/session.rs
M rust-rewrite-stages.md
?? crates/xo-syncd/tests/
?? crates/xo/src/lib.rs
?? examples/
```

Do not reset, restore, or overwrite these files. They contain the complete set
of guided `xo-syncd` pairing, example systemd-unit, and multi-client E2E
changes.

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

- `xo.scm` is the only projected workspace configuration filename.
- If the loaded workspace behavior has no views, `WorkspaceSession::behavior`
  creates and replicates native declarative `xo.scm` with:
  - `notes`: `type == "note"`, newest first
  - `all`: no predicate, newest first
- Native workspace forms cover views, subviews, predicates, actions and effects,
  templates, capability grants, and query limits.
- The core native loader rejects JSON descriptor envelopes and `exo.scm`.
  `WorkspaceSession` recognizes only the exact JSON envelope emitted by an
  earlier prerelease Rust iteration, validates it, and immediately commits and
  projects the equivalent native form. This one-time development-state repair
  fixes existing WIP workspaces without widening the native configuration
  language.
- Arbitrary executable Steel forms remain outside the declarative sandbox
  boundary.

### TUI layout and controls

The screen currently contains:

- a four-line header;
- connection/workspace state in the first two header lines;
- key hints arranged in three columns in the next two header lines;
- a toggleable left tag pane;
- a middle filtered-note pane;
- a right raw-Markdown preview pane.

Important controls:

- `Tab`/`Shift-Tab`: change pane
- Left/Right or `h`/`l`: move spatially between visible panes
- arrows or `j`/`k`: move through the focused list
- `Space` or `Enter` in the tag pane: toggle the highlighted tag filter
- `/`: open the title-filter input between header and content
- `g`: open the view/subview goto menu and type its displayed unique prefix
- `T`: show or hide the tag pane
- `c`: prompt for a title, construct default frontmatter, then open `$EDITOR`
- `Enter` or `e` on a note: edit it
- `d`/`u`: delete/restore
- `a`: fuzzy action picker
- `x`, `v`, `y`: conflicts, devices, and synchronization details
- `J`: open the guided `xo-syncd` pairing wizard
- `r`: refresh/retry synchronization
- `q`: quit

Selection is clamped to the currently visible note list. This fixes the earlier
failure where filtering left an invalid index and preview/edit appeared broken.
Tag counts are faceted in real time: they respect the current view/subview and
title query, and each count includes the current tag filters plus that tag.

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

### Recursive import and conventional export

- `xo import SOURCE [--type TYPE]` operates on the configured active workspace.
  It recursively scans outside the active projection, completes all Markdown,
  duplicate-ID, and active-workspace collision checks before committing, adds
  required fields, leaves the source unchanged, and materializes the imported
  winning revisions.
- `xo export DESTINATION [--type TYPE]` exports winning revisions only to a new
  or empty destination. It groups files by type/year/month, strips ordinary
  internal metadata, allocates deterministic filename suffixes, preserves IDs
  for encrypted bodies, and never overwrites existing files.
- `crates/xo/tests/import_export.rs` invokes the real binary for a recursive
  import/export round trip. Pure model tests cover malformed input, collisions,
  filtering, encrypted output, and destination refusal.

URL capture/readable-content conversion is explicitly reserved for a
capability-gated Steel action plugin. Its network/readability implementation
must sit behind a testable native host service; do not grant ambient network
access to the Steel runtime or hard-code URL capture into the TUI.

## Synchronization server workflow

`README.md` documents the implemented setup:

1. Seed a server workspace with `xo-admin import-workspace`.
2. Start `xo-syncd` against that state directory.
3. Initialize the client config with `xo config-init`.
4. Join using `xo --ticket '<WRITABLE_TICKET>'`.

The README also includes systemd, health/status/metrics, additional clients,
backups, and headless attachment of a server to a TUI-created workspace.

Important operational constraint: never open the same state directory
concurrently with `xo`, `xo-admin`, and `xo-syncd`.

Live synchronization now resumes without re-supplying a ticket:

- `IrohWorkspace::resume_sync` starts Iroh Docs synchronization with its
  persisted useful-peer list.
- The TUI calls it whenever it reopens an existing workspace without a ticket.
- `xo-syncd` calls it for every stored workspace before serving operator
  requests.
- `two_peers_sync_and_second_peer_survives_restart` verifies that a restarted
  peer receives a revision written after restart.

`xo-admin import-ticket STATE_DIR TICKET` imports a writable capability without
launching the TUI and prints both the workspace ID and a server-addressed
writable ticket. Read-only tickets are rejected before an Iroh state directory
is created. Repeating the same import is safe. The returned server ticket is
used once on the original client to establish the bidirectional peer
relationship before normal restart-based synchronization takes over.

The TUI now drives that exchange with `J`:

- it asks for the server state directory;
- generates a writable client invitation;
- shows the workspace ID, loopback setup URL, and operator-token location;
- copies the invitation through OSC 52 or reveals it on explicit request;
- directs the user to the authenticated `xo-syncd` setup page, which validates
  the workspace ID and writable capability before importing it;
- retains POSIX-quoted stop/import/start commands behind `C` as a headless
  fallback;
- accepts the returned server ticket, complete page output, or a `ticket=` line;
- validates and connects the returned server ticket; and
- zeroizes the in-memory invitation, pasted output, and clipboard payload.

The daemon serves its setup form at `/setup`. It imports the matching writable
ticket through the live Iroh node, starts synchronization, adds the workspace
to authenticated status/metrics output, and returns a server-addressed ticket.
The form is protected by the operator token and does not persist submitted
secrets in browser storage.

Projected note paths are canonical:
`<first-three-ID-characters>/<ID>-<title-slug>.md`. The scanner,
materializer, TUI mutation path, and Markdown importer all use the same helper;
title changes therefore move the projection while retaining the stable note
identity.

Example system and user units live below `examples/systemd/`.

The root `Dockerfile` builds only `xo-syncd` in a Rust 1.89 builder and runs it
as UID/GID 10001 with `/data` as its durable volume. The operator endpoint binds
inside the container on port 9464; deployment examples publish it only to host
loopback. `.github/workflows/build.yml` now runs Rust formatting, strict Clippy,
the complete workspace tests, native Linux/macOS release builds, and a
multi-platform syncd-only GHCR image build.

The native release matrix covers Linux x86-64, Linux ARM64, macOS ARM64, and
Windows x86-64. Any pushed tag creates a GitHub Release after all four
archives succeed, attaching those archives plus `SHA256SUMS` and generated
release notes.

`syncd_restart_converges_two_restarted_tui_clients_with_offline_conflict` is a
process-level E2E test. It launches the compiled `xo-syncd`, joins two
independent TUI sessions, restarts the daemon and both clients, creates
concurrent offline edits, and verifies convergence and retained history in both
clients and the daemon's persisted workspace.

## Known limitations and likely next work

### 1. Template creation UX needs a decision

The core supports templates, but `c` now follows the requested default
frontmatter/title/editor flow. If templates return to the TUI, add an explicit
template picker rather than silently choosing the first template.

### 2. Remaining roadmap work

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
- `crates/xo/src/lib.rs` — narrow library boundary exposing the real TUI session to E2E tests
- `crates/xo/src/app.rs` — TUI model, filtering, faceted tags, goto menu, preview,
  highlighting, rendering, and most TUI tests
- `crates/xo/src/config.rs` — native command configuration
- `crates/xo/src/content_io.rs` — recursive import preflight and deterministic conventional export
- `crates/xo/src/session.rs` — local workspace selection, projection hydration,
  behavior bootstrap, persistence, and Iroh-backed TUI tests
- `crates/xo-core/src/iroh_node.rs` — persistent endpoint and workspace sync API
- `crates/xo-core/src/steel_runtime.rs` — sandboxed Steel and config encoding
- `crates/xo-core/src/projection.rs` — projection validation/materialization
- `crates/xo-core/src/workspace_projection.rs` — bidirectional workspace/projected
  file handling
- `crates/xo-core/src/watcher.rs` — supported projected path watching
- `crates/xo-syncd/src/main.rs` — daemon startup and persistent Iroh peer
- `crates/xo-syncd/tests/multiple_tui_clients.rs` — real-daemon, two-TUI restart and conflict E2E
- `crates/xo-syncd/src/operator.rs` — health, status, and metrics HTTP API
- `crates/xo-admin/src/main.rs` — import, invitations, backups, diagnostics,
  retirement, and namespace rotation
- `README.md` — user/operator setup guide
- `examples/systemd/` — system-wide and per-user `xo-syncd` units
- `rust-rewrite-stages.md` — implementation checklist and test gates
- `exo-rs-plan.md` — original rewrite plan
- `oldcodebase/` — legacy implementation and compatibility source

## Last verified state

After the recursive import/export changes, focused verification is:

```text
cargo fmt --all -- --check
passed

cargo clippy --workspace --all-targets -- -D warnings
passed

cargo test -p xo
31 passed; 0 failed

git diff --check
passed
```

The current `cargo test --workspace` run passes all new import/export tests but
fails in the unrelated existing
`backup::tests::restored_peer_serves_blobs_and_rejoins_an_active_peer` test
while reading an Iroh entry blob:

```text
Io(Kind(NotFound)): entity not found
```

The failure reproduces with the feature-enabled backup test in isolation. The
prior full-workspace verification after the real-daemon, multi-TUI E2E changes
was:

```text
cargo fmt --all -- --check
passed

cargo clippy --workspace --all-targets -- -D warnings
passed

cargo test -p xo-admin
5 passed; 0 failed

cargo test --workspace
101 passed; 0 failed

git diff --check
passed
```

The multi-peer Iroh portion of the workspace suite takes roughly two minutes
and requires local socket access.

## Handoff hygiene

- No tickets, operator tokens, endpoint keys, or other secrets are recorded in
  this document.
- Do not use destructive Git commands on the dirty working tree.
- Prefer focused tests while iterating, then run the full workspace suite.
- Keep README limitations aligned with actual behavior; especially remove the
  repeated-ticket warning only after restart/resume synchronization is tested.
