# xo Rust + Iroh architecture

## Product Summary

xo is an offline-first personal knowledge system for Markdown notes with YAML frontmatter. Every native device maintains a readable Markdown projection while an Iroh Docs workspace is the authoritative replicated dataset.

Iroh supplies authenticated, end-to-end encrypted QUIC connections, preferring direct peers and falling back to relays. Iroh Docs provides eventually consistent multiwriter key-value replication; Iroh Blobs stores note and asset bytes, and Iroh Gossip provides live replication. [Iroh overview](https://docs.iroh.computer/), [Iroh Docs crate](https://docs.rs/iroh-docs/latest/iroh_docs/)

## Product Capabilities

### Knowledge model

- Markdown documents with YAML frontmatter, stable lowercase IDs, wikilinks, tags, and arbitrary content types.
- Recursive human-friendly folder organization with Markdown as a materialized local projection.
- Template-based note creation with automatic IDs, slugs, dates, and times.
- URL capture, webhook ingestion, image attachments, Goodreads import, Hardcover search, reading states, statistics, and encrypted notes.
- Conventional Markdown import and export.

### Workspace behavior

Steel Scheme defines synchronized workspace behavior.

- The root `xo.scm` file declares views, actions, templates, and defaults; modules live under `modules/` as `.scm` files.
- Views define display names, shortcuts, predicates, sorting, title/subtitle fields, preview templates, tag visibility, and statistics templates.
- Subviews add named filtering predicates.
- Actions define a label, applicability predicate, granted capabilities, and a note transformation.
- URL capture and readable-content conversion are delivered as a Steel action plugin. Network fetching and readability run through an explicitly granted, testable native host capability; Steel itself receives no ambient network access.
- Scripts receive a flat note value with frontmatter fields, id, path, and body.
- The host provides deterministic tag, date, ID, link, and bounded-query helpers.
- Steel runs in a capability sandbox without filesystem, network, process, wall-clock, or secret access unless explicitly granted.

[Steel](https://github.com/mattwparas/steel) is an embeddable Rust Scheme with modules, macros, immutable data structures, and Rust integration. Pin an exact pre-1.0 release behind a narrow host adapter.

## Interfaces

### TUI

The TUI is the full-featured application.

- View selection and shortcuts; built-in unfiltered All view; subview tabs.
- Multi-select tag filtering, title search, Markdown preview, and pane navigation.
- Create, edit, delete, and externally edit notes.
- Fuzzy action picker, imports, exports, books, URL capture, webhooks, image attachments, diagnostics, and conflict resolution.
- Sync state, durable-operation inspection, retry controls, and device management.
- Optional LSP companion with wikilink/tag completion, preview, navigation, references, rename, diagnostics, semantic tokens, and code actions.

### PWA

The PWA is an incidental-access client using Iroh WASM bindings and IndexedDB-backed local state.

- Browser Iroh identity, workspace tickets, QR-based TUI onboarding, offline cache, durable operations, sync status, reconnection, and conflict indication.
- Browse views and subviews, search titles, filter tags, read sanitized Markdown, and view attachments.
- Create quick/inbox notes; edit/delete notes; edit YAML frontmatter; attach images; capture URLs.
- Encrypt and unlock note bodies in the browser.
- Present safe declarative action and view descriptors.
- Exclude Steel execution, arbitrary custom actions, bulk import/export, advanced book workflows, workspace configuration authoring, external-editor integration, and LSP features.

A browser feasibility spike validates Iroh WASM, Docs, Blobs, Gossip, IndexedDB persistence, background reconnect behavior, and relay fallback.

### iOS application

The iOS app is SwiftUI backed by a Rust core through UniFFI or a thin C/Swift FFI layer.

- Native Iroh endpoint and direct Docs, Blobs, and Gossip replication.
- Encrypted application-container state and foreground/permitted background sync.
- Inbox capture, reading, search, note editing, tags, wikilinks, attachments, book status, notifications, device identity, and conflict review.
- Safe workspace descriptors rather than arbitrary Steel execution.

### Headless central sync peer

A Rust daemon with no end-user interface.

- Durable Iroh Docs/Blobs replica and always-available bootstrap peer.
- Health, metrics, encrypted backup/restore, and authenticated operator endpoints.
- CLI/config administration, relay configuration, stale-device reporting, and conflict reporting.
- No ordering authority: the peer is a durable participant in replicated workspace state.

## Canonical Storage and Sync Design

### Workspace namespace

Each workspace is one Iroh Docs namespace. Invitations include a Docs ticket and a descriptor containing the workspace ID, namespace capability, bootstrap peers, relay/discovery configuration, schema version, and an optional encrypted workspace-key envelope.

Every device has a distinct Iroh endpoint identity and Docs author identity. Read-only invitations never contain a write capability.

### Records

Use immutable revision blobs with compact Docs entries:

~~~
note/<note-id>/<revision-id>       -> BLAKE3 hash of a NoteRevision blob
note/<note-id>/head/<author-id>    -> author latest asserted revision
asset/<asset-id>                   -> BLAKE3 hash, MIME, size
config/<path>/<revision-id>        -> BLAKE3 hash of Steel configuration bytes
tombstone/<target-id>/<author-id>  -> deletion revision
device/<endpoint-id>               -> label, capabilities, last-seen metadata
~~~

A NoteRevision contains full frontmatter, body, materialized path, HLC, author ID, predecessor revision IDs, and deletion state. The visible head is resolved by highest HLC, then actor ID. Deleted heads hide the note; non-winning concurrent revisions remain visible as history and conflicts.

Iroh Docs replicates revision references; xo owns content resolution and conflict presentation rather than attempting character-level Markdown merging.

### Blobs, Docs, and Markdown projection

- Iroh Blobs holds revisions, attachments, Steel configuration, and large imports.
- Iroh Docs holds compact references and replication metadata.
- Native clients materialize winning heads into Markdown and Steel files.
- Filesystem changes create immutable revision blobs and update the local device head.
- Remote projection writes are suppressed by the watcher to avoid duplicate revisions.
- Keys, caches, diagnostics, materialization hashes, and Blob cache state remain outside the exported workspace.

Iroh Docs uses a storage abstraction. Native clients use persistent local storage; browser clients use a WASM-compatible IndexedDB store. [Iroh Docs storage model](https://docs.rs/iroh-docs/latest/iroh_docs/)

## Rust Components

### xo-core

Shared domain and sync layer.

- Markdown/YAML codec, schema validation, IDs, tags, wikilinks, encryption, imports/exports, and materialization.
- Steel runtime host, module loader, view evaluation, action execution, templates, capability checks, and diagnostics.
- Iroh Endpoint, Docs, Blobs, Gossip, tickets, identities, invitations, revocations, discovery, and sync diagnostics.
- Revision resolution, tombstone handling, conflict detection, blob availability, and filesystem projection.
- Stable async APIs for TUI, server, Swift bindings, WASM bindings, and LSP.

### Supporting binaries

- xo: TUI and local workspace peer.
- xo-lsp: editor integration.
- xo-syncd: headless replication peer.
- xo-admin: invitations, device retirement, backup/restore, diagnostics, and relay configuration.

## Security and Operations

- Endpoint keys identify devices; Docs author keys identify writes; invitations distribute workspace capability.
- Sensitive bytes are encrypted before entering Iroh Blobs, so peers and relays store or route ciphertext.
- Signed revocation records retire devices without erasing historical revisions.
- Backups include Docs metadata, verified Blob content, and materialized Markdown.
- Metrics cover reachability, direct-versus-relay ratio, convergence, blob availability, disk use, conflicts, and stale devices.
- Unknown newer workspace schemas open read-only.

## Delivery Sequence

1. Build xo-core with codecs, materialization, revision resolution, local index, and a two-native-peer Iroh proof of concept.
2. Add the Steel host, sandbox, configuration schema, view evaluation, action execution, and diagnostics.
3. Deliver the TUI and central peer with invitations, offline/direct sync, relay fallback, Blob transfer, and backups.
4. Deliver LSP, imports/exports, books, URL/webhook workflows, attachments, and conflict resolution.
5. Deliver the iOS app through Rust FFI and native P2P synchronization.
6. Deliver the PWA after the Iroh WASM interoperability spike succeeds.

## Validation

- Concurrent edits, deletion, rename, and restore converge to the same visible heads while retaining conflict history.
- Offline edits synchronize over direct peers and relay fallback.
- Blobs verify by BLAKE3 hash and resume after interruption.
- Markdown materialization round-trips without watcher loops or duplicate revisions.
- TUI, iOS, and PWA interoperate on one namespace; browser tests verify IndexedDB persistence and reconnect behavior.
- Steel scripts are capability-isolated and produce scoped diagnostics on failure.
- Revoked devices cannot publish future heads.
- Backup restore reproduces documents, blobs, Steel configuration, and the Markdown projection.
