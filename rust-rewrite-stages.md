# Rust Rewrite Implementation Stages

This checklist tracks the greenfield Rust implementation roadmap. The Go
codebase and its on-disk formats are non-normative: no source, configuration,
ID, encryption, or behavior compatibility is required. A stage is complete
only when every task and its exit gate are checked.

## Current status

- Stage 0 is in progress: native persistence and capabilities pass; relay-only validation remains.
- Stage 1 is in progress: validation and immutable revision operations exist; shared service APIs and broader property tests remain.
- Stage 2 is complete: authoritative notes, assets, configuration, indexing, and projection are implemented.
- Stage 3 is complete: native peers converge across partitions and restarts, retain conflicts, and resume verified partial Blob transfers.
- Stage 4 is complete: central-peer operations, backup/restore, signed retirement, and namespace rotation/reinvitation pass their security and recovery scenarios.
- Stages 5 and 6 are complete: portable sandboxed Steel behavior and the persistent daily-use Ratatui client pass their offline, conflict, and convergence gates.
- Stage 7 is in progress: import/export, capability-gated URL capture, reading states, and the executable Steel Hardcover plugin are complete; webhook, encrypted-note, Goodreads-plugin, and statistics workflows remain. Image attachments are out of scope.
- Stage 8, the full offline-first PWA, is the next priority.
- Stage 9 retains the deferred LSP work. There is no iOS application stage.

## Stage 0 — Architecture proof

- [x] Create the Cargo workspace with `xo-core`, `xo`, `xo-syncd`, `xo-admin`, and `xo-lsp` crates.
- [x] Pin a Rust 1.89-compatible Iroh, Docs, Blobs, and Gossip dependency set in `Cargo.lock`.
- [x] Pin Steel behind an optional `xo-core` feature.
- [x] Define schema-v1 record keys, canonical CBOR values, BLAKE3 revision IDs, HLC ordering, and revocation semantics.
- [x] Compose persistent Iroh Docs, Blobs, and Gossip on a single endpoint and router.
- [x] Persist endpoint and Docs author identities.
- [x] Prove two native peers can exchange a writable ticket and replicate content.
- [x] Prove a replicated peer retains its endpoint identity, namespace, metadata, and blob content after restart.
- [ ] Prove synchronization using relay fallback with direct connectivity disabled.
- [x] Add an automated read-only ticket test proving writes are rejected.

**Exit gate**

- [ ] Two persistent peers exchange revisions and assets over direct and relay-only paths, restart, reconnect, and retain identical verified state.

## Stage 1 — Deterministic domain core

- [x] Define workspace, actor, note, revision, asset, head, tombstone, device, conflict, and schema-version types.
- [x] Implement deterministic CBOR revision serialization and BLAKE3 revision identities.
- [x] Implement HLC local advancement, remote observation, clock-regression handling, and actor tie-breaking.
- [x] Implement deterministic visible-head selection and concurrent-head conflict reporting.
- [x] Distinguish predecessor history from concurrent conflicts.
- [x] Implement Markdown/YAML parsing and deterministic frontmatter map ordering.
- [x] Implement stable lowercase ID generation and validation.
- [x] Implement tag extraction, slug generation, and wikilink parsing.
- [x] Implement a versioned Argon2id/AES-256-GCM envelope with note-ID associated data.
- [x] Add schema validation for every replicated record type.
- [x] Reject non-finite numeric frontmatter before canonical serialization.
- [x] Add explicit deletion, restore, and rename domain operations.
- [x] Add revision-graph validation for missing or cross-note predecessors.
- [x] Add template variables and deterministic date/time helper inputs.
- [ ] Add storage-independent async services for queries, mutations, history, conflicts, assets, and diagnostics.
- [x] Add deterministic permutation tests proving resolution is independent of head arrival order.

**Exit gate**

- [ ] Property tests prove deterministic resolution, Markdown round trips, stable IDs, and encrypted-envelope correctness.

## Stage 2 — Persistent local workspace and projection

- [x] Add a disposable SQLite index for IDs, paths, titles, types, tags, hashes, and diagnostics.
- [x] Rebuild the complete index from authoritative Docs and Blob state.
- [x] Materialize winning note heads into recursive Markdown paths.
- [x] Canonicalize projected notes as `<first 3 chars of id>/<id>-<title-slug>.md` across scanning, mutation, import, and materialization.
- [x] Materialize assets below the workspace `assets/` directory.
- [x] Perform atomic same-directory projection writes.
- [x] Retain expected-write hashes for crash recovery and watcher suppression.
- [x] Add a recursive debounced filesystem watcher.
- [x] Ignore `.exo/` and other hidden local-only directories while scanning.
- [x] Suppress watcher events caused by remote materialization.
- [x] Convert local creates, edits, deletes, and renames into immutable revisions.
- [x] Diagnose duplicate IDs and malformed Markdown without silently rewriting them.

**Exit gate**

- [x] Authoritative workspace state round-trips through a clean projection without data loss, duplicate revisions, or watcher loops.

## Stage 3 — Native multi-peer synchronization

- [x] Support persistent Iroh endpoint and Docs author identities.
- [x] Create and import read-write workspace tickets.
- [x] Replicate generic Docs values and associated Blob content between two peers.
- [x] Add exokephalos-specific immutable revision and per-author head repositories.
- [x] Add verified asset metadata and Blob repositories.
- [x] Add configuration, tombstone, and device record repositories.
- [x] Add read-only invitation creation and enforcement tests.
- [x] Add bootstrap-peer and discovery configuration.
- [x] Track durable operations, retries, missing blobs, connectivity, and convergence state.
- [x] Add offline edit and reconnect synchronization.
- [x] Add three-peer partition and convergence tests.
- [x] Test concurrent edit, delete/edit, rename/edit, and restore scenarios.
- [x] Test interrupted Blob transfer and resume.
- [x] Expose stable command and event APIs for native frontends.

**Exit gate**

- [x] Three independently persisted peers converge after partitions and restarts while retaining losing concurrent revisions.

## Stage 4 — Central peer, backup, and device security

- [x] Start `xo-syncd` as a persistent Iroh Docs/Blobs/Gossip protocol host.
- [x] Add structured logs, health checks, metrics, and authenticated operator endpoints.
- [x] Add invitation, device-list, retirement, and diagnostics commands to `xo-admin`.
- [x] Add headless writable-ticket import for attaching a stopped server state directory to an existing workspace.
- [x] Add relay administration commands to `xo-admin`.
- [x] Enforce signed normal-retirement cutoffs while reading author records.
- [x] Implement hard revocation through namespace rotation and reinvitation.
- [x] Implement verified backup creation.
- [x] Implement restore into a clean state directory.
- [x] Verify restored peers can serve every referenced Blob and rejoin active peers.

**Exit gate**

- [x] A clean machine restores a backup, serves all blobs, converges with an active peer, and ignores post-retirement writes.

### Stage 4 test coverage

- `verified_backup_restores_exact_bytes_and_rejects_corruption` verifies every manifest size and BLAKE3 digest, restores identical bytes, and rejects a corrupted payload.
- `restored_peer_serves_blobs_and_rejoins_an_active_peer` restores a clean peer, reads its backed-up Blob, reconnects it, confirms a later signed revision physically replicated, and proves the retirement cutoff keeps the earlier accepted revision visible.
- `signed_retirement_ignores_later_writes_and_retains_history` creates an offline post-cutoff write, reconnects both peers, and proves the signed write is retained in transport but excluded from resolution.
- `rotation_reinvites_active_peer_and_excludes_retired_peer` checkpoints accepted notes, assets, and configuration into a fresh namespace, reinvites an active peer, denies the retired peer access, and proves writes to the archived namespace cannot enter the rotated namespace.
- `ticket_import_is_idempotent_and_resumes_after_restart` rejects read-only capabilities before creating state, safely repeats writable imports, returns a server-addressed ticket, and proves replication resumes after both peers establish their durable relationship.
- The `xo-syncd` operator tests verify token creation and reuse, reject weak tokens, require authentication for status and metrics, validate Prometheus output, and exercise the live HTTP listener.

## Stage 5 — Steel workspace behavior

- [x] Pin Steel behind the `xo-core/steel` feature.
- [x] Define the native `xo.scm` and `modules/**/*.scm` configuration schema.
- [x] Encode workspace behavior entirely as native declarative Steel forms.
- [x] Implement the narrow Steel host adapter and deterministic helper functions.
- [x] Deny ambient filesystem, network, process, secrets, and wall-clock access.
- [x] Implement views, subviews, sorting, previews, actions, templates, bounded queries, and capability grants.
- [x] Produce safe declarative descriptors for clients that cannot execute Steel.
- [x] Synchronize and materialize versioned Steel configuration records.

**Exit gate**

- [x] Workspace views and actions execute deterministically, and sandbox tests prove unavailable capabilities cannot be reached.

### Stage 5 test coverage

- `sandbox_and_schema_reject_ambient_capabilities` proves filesystem, environment/secrets, process, network, dynamic evaluation, and ambient clock forms cannot cross the descriptor boundary; the clock helper returns only the caller-supplied value.
- `loads_and_merges_native_modules` verifies deterministic lexical module merging and client-safe serialization.
- `native_workspace_config_round_trips_every_descriptor_field` covers native views, predicates, subviews, actions, every effect/value form, templates, grants, query limits, and rejection of the obsolete JSON envelope.
- `synchronized_config_is_verified_and_materialized` verifies replicated configuration hashes, projection, watcher suppression, and immutable local configuration revisions.

## Stage 6 — Daily-use TUI

- [x] Reserve the `xo` binary and basic command-line parsing.
- [x] Build the Ratatui application shell and pane navigation.
- [x] Add the built-in All view and configured view/subview navigation.
- [x] Add title search, multi-tag filtering, sorting, and Markdown preview.
- [x] Add create, edit, delete, restore, templates, and external-editor integration.
- [x] Add the fuzzy action picker.
- [x] Show sync state, durable operations, retry controls, missing blobs, and diagnostics.
- [x] Add conflict history, conflict review, and device management.
- [x] Add secure encrypted-note preview and temporary-file editing.
- [x] Replace the footer with a four-line connection/key-hint header.
- [x] Lay out header shortcuts in three columns and show tags, filtered notes, and preview in the three content panes.
- [x] Add inline `/` filtering and a `g` goto menu with computed unique prefixes for views and subviews.
- [x] Make the tag pane toggleable with `T` and show live faceted counts for the current view, title query, and selected tags.
- [x] Bootstrap `xo.scm` with Notes and All views when no views exist.
- [x] Guarantee `id`, `created`, `tags`, `title`, and `type` on newly created items.
- [x] Show the serialized raw Markdown document in preview with lightweight syntax highlighting.
- [x] Prompt for a title before creation and open the complete new document in the external editor.
- [x] Add a TUI-guided `xo-syncd` pairing flow with safe command generation, hidden tickets, server-output parsing, and durable peer connection.
- [x] Add an authenticated browser setup page to `xo-syncd` and make workspace ID, ticket transfer, and the returned server ticket the primary TUI pairing flow.

**Exit gate**

- [x] A user can use the workspace offline as their primary TUI, reconnect, review conflicts, and converge with `xo-syncd`.

### Stage 6 controls and test coverage

- `xo` opens directly in TUI mode. It reads `~/.config/xo/config.scm`; `xo config-init` prints the default document, while a fresh state directory creates a local workspace automatically. `--state-dir`, `--workspace`, and `--projection` override persistent defaults; the non-persistent `--ticket` option is used only to join an Iroh invitation.
- `config_init_output_starts_a_fresh_xo_workspace` runs the real `xo config-init` command, loads its output through the startup configuration path, opens a fresh persisted session, and verifies the native Notes/All workspace configuration is projected.
- `Tab`/`Shift-Tab` cycle between visible panes; Left/Right and `h`/`l` move spatially between them; Up/Down and `j`/`k` navigate the focused list; `Space`/`Enter` toggles a highlighted tag; `g` opens the view/subview menu and its displayed unique prefixes navigate directly; `/`, `T`, and `s` control title filtering, tag-pane visibility, and sorting.
- `c` prompts for a title and opens the initialized item in `$EDITOR`; `Enter`/`e`, `d`, and `u` edit, delete, and restore; external editors run in a restored normal terminal; `a` opens the fuzzy action picker; `p` unlocks encrypted preview; `x`, `v`, and `y` open conflicts/history, devices, and sync details; `r` refreshes and `R` retries a durable operation.
- `offline_tui_edit_reconnects_retains_conflict_and_converges` takes the primary peer offline, commits independent primary and central edits, reconnects, proves both peers retain the conflict, and verifies the immutable history converges.
- `tui_pairing_invitation_connects_a_sync_peer` follows the TUI pairing APIs, imports the invitation into a server peer, returns its ticket, and proves a server write reaches the client.
- `syncd_restart_converges_two_restarted_tui_clients_with_offline_conflict` launches the real `xo-syncd` binary, connects two independently persisted TUI sessions, restarts every participant, creates concurrent offline edits, and verifies both client histories plus the daemon's persisted conflict.
- Pairing model/render tests cover the browser-first setup instructions, POSIX-safe fallback commands, complete-output and direct-ticket parsing, every wizard step, and default ticket redaction.
- Operator tests cover the public setup form, authentication, URL-encoded fields, workspace/ticket mismatch rejection without import, writable-ticket import, live synchronization startup, and server-ticket return.
- Ratatui `TestBackend` tests cover the four-line, three-column header; view/search/tag-filter-aware faceted counts; tag-pane visibility; valid note selection; inline filtering and prefix goto navigation; metadata-rich preview; actions; deletion/restore; and encrypted temporary-file editing. The editor regression test also covers editors that atomically replace the temporary file.

## Stage 7 — Content workflows

The remaining Stage 7 workflows are deferred behind the Stage 8 PWA foundation
unless the PWA directly depends on them.

- [x] Implement recursive Markdown import and conventional Markdown export.
- [x] Implement URL capture and readable-content conversion as a capability-gated Steel action plugin, backed by a testable native host service rather than ambient Steel network access.
- [ ] Add optional authenticated webhook ingestion to `xo-syncd`.
- [ ] Complete encrypted-note user workflows.
- [ ] Implement Goodreads import as a capability-gated executable Steel plugin.
- [x] Implement Hardcover search as a capability-gated executable Steel plugin.
- [x] Implement `to-read` → `reading` → `read` state actions in Steel.
- [ ] Add reading statistics.
- [ ] Place external integrations behind testable traits and fixtures.

Import/export coverage includes recursive discovery, source immutability,
required-field normalization, full diagnostic preflight, active-workspace
identity/path collisions, type-filtered conventional output, deterministic
filename collisions, encrypted-ID preservation, refusal to overwrite a
non-empty destination, and a real `xo import` → `xo export` process test.

URL capture is declared by the native `(capture-url)` Steel plugin and requires
both `create-note` and `network` grants. The native host pins validated public
DNS results, revalidates redirects, limits status/content type/body size, runs
readability extraction, converts links to absolute Markdown, and commits the
result through the ordinary revision path. Fixture fetchers and extractors keep
network and conversion behavior testable without ambient Steel access.

**Exit gate**

- [ ] Every supported workflow has deterministic fixtures and produces ordinary replicated revisions or records.

## Stage 8 — Offline-first PWA (next priority)

The PWA is a first-class client, not a compatibility layer. Rust remains
authoritative for records, revisions, conflicts, encryption, behavior, and
synchronization. A dedicated Web Worker owns Rust, Steel, Iroh, and IndexedDB
coordination; the UI thread owns only presentation and browser interaction.

- [x] Establish the static React/PWA shell, nginx `xo-web` image, typed dedicated-worker RPC, IndexedDB checkpoint probe, and sandboxed Steel Wasm probe.
- [ ] Split browser-safe `xo-core` features from native filesystem, process, RPC, and Tokio networking dependencies.
- [ ] Add a `xo-web` Wasm crate with a small message-based API for workspace lifecycle, queries, mutations, events, sync status, conflicts, encryption, and executable Steel actions.
- [ ] Run an Iroh browser feasibility spike covering relay-only connectivity, Docs, Blobs, Gossip, native namespace interoperability, and restart recovery.
- [ ] Use direct browser Iroh when feasible; if an upstream browser limitation blocks it, document the evidence and implement a narrow WebSocket/HTTPS `xo-syncd` transport without moving revision or action authority to the server.
- [ ] Persist browser identity, encrypted capabilities, verified records, blobs, pending writes, tombstones, and trusted plugin hashes in IndexedDB.
- [ ] Rebuild disposable indexes and UI projections from verified IndexedDB state after startup or interruption.
- [ ] Run each arbitrary Steel action in a disposable sandboxed worker with explicit browser host capabilities, time/size limits, and termination support.
- [ ] Provide capability-gated browser host services for fetch, secrets, note creation/mutation, clipboard, and notifications; expose no ambient network, filesystem, process, socket, or dylib access.
- [ ] Build the installable React PWA for reading, search, capture, editing, tags, wikilinks, books, conflicts, devices, and synchronization diagnostics.
- [ ] Keep the service worker limited to versioned application-shell caching; workspace durability belongs to IndexedDB and Rust recovery logic.
- [ ] Support offline creation and edits, queued synchronization, relay reconnect, conflict review, and convergence with native peers and `xo-syncd`.
- [ ] Add browser tests for interrupted writes, worker termination, plugin capability denial, encrypted-note handling, upgrades, offline restart, and native/browser convergence.
- [ ] Add installability, accessibility, responsive-layout, and supported-browser checks.

Image attachments and native-only OS integrations are explicitly out of scope.

**Exit gate**

- [ ] An installed PWA works offline, survives browser and worker restarts, runs capability-gated arbitrary Steel, reconnects through the available Iroh/`xo-syncd` transport, and converges with native peers without server-side action execution.

## Stage 9 — LSP companion (deferred until after the PWA)

- [x] Reserve the `xo-lsp` binary crate.
- [ ] Implement stdio LSP lifecycle and workspace loading.
- [ ] Add wikilink and tag completion.
- [ ] Add hover, definitions, references, document links, and symbols.
- [ ] Add rename across multiple Markdown files.
- [ ] Add diagnostics, semantic tokens, inlay hints, and code actions.
- [ ] Route editor writes through the normal watcher and revision pipeline.

**Exit gate**

- [ ] LSP tests pass and multi-file rename creates valid revisions without watcher duplication.

## Repository-wide verification

- [ ] `cargo test --workspace --all-targets` passes consistently. The latest full run passed all new workflow and Steel-plugin tests but hit the existing flaky `two_peers_sync_and_second_peer_survives_restart` Iroh replication failure.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` passes.
- [x] `cargo fmt --all -- --check` passes.
- [x] `git diff --check` passes.
- [ ] Linux CI passes.
- [ ] macOS CI passes.
- [ ] Windows CI passes before declaring the TUI stable.
- [ ] Browser/Wasm CI passes before declaring the PWA stable.
