# xo Rust + Iroh architecture

## Product summary

xo is an offline-first personal knowledge system for Markdown notes with YAML frontmatter. An Iroh Docs namespace is the authoritative replicated workspace. Native clients materialize its winning note and configuration revisions as readable files; `xo-web` resolves the same records directly in Rust/WebAssembly.

Iroh provides authenticated end-to-end encrypted connections, direct connectivity for native peers, relay fallback, Docs replication, Blob transfer, and Gossip-assisted live synchronization. Browser peers are intentionally relay-only.

Image attachments and a native iOS application are not in scope.

## Knowledge and behavior

- Notes have current-format seven-character IDs, recursive YAML-compatible frontmatter, Markdown bodies, canonical projected paths, immutable revisions, per-author heads, and retained conflict history.
- Native workspaces support conventional Markdown import/export, URL capture, Hardcover workflows, reading states, encrypted note bodies, wikilinks, and tags.
- Replicated `xo.scm`, `modules/**/*.scm`, and `plugins/**/*.scm` define views, subviews, templates, actions, and explicit capability grants.
- Predicates, sorting, searches, tags, and declarative effects execute through shared Rust behavior code.
- Steel runs without ambient filesystem, process, socket, environment, secret, or dynamic-library access. Host capabilities are explicit and validated.

## Interfaces

### `xo` TUI

The native TUI uses strict schema-3 command configuration; `leader-key` is one printable character and defaults to Space. It provides:

- an uncluttered release-only header, a configurable leader-key popup for operational commands, and a compact footer for leader/search/edit/create/delete/restore/quit hints;
- view and subview navigation, title search, conjunctive tag filters, sorting, note lists, and Markdown preview;
- note creation, frontmatter/body editing, deletion, restoration, revision/conflict inspection, and encrypted-note unlocking;
- generic action selection, URL capture, executable Steel plugins, import/export, diagnostics, retries, and device management;
- server pairing and writable mobile onboarding through a fragment-based QR setup URL; and
- filesystem projection plus optional `xo-lsp` editor integration.

### `xo-web` PWA

`xo-web` is a static, installable application. React owns presentation while a dedicated worker owns Rust/Wasm, Steel, Iroh, WebCrypto coordination, and IndexedDB recovery.

- It creates or joins writable workspaces and synchronizes directly with native peers through relay-only Iroh Docs/Blobs/Gossip.
- It loads and validates replicated Steel configuration, displays configured views and subviews, and runs view, search, sort, and tag queries in shared Rust behavior code.
- It creates and edits frontmatter plus Markdown, deletes and restores notes, displays revision history and concurrent heads, and commits canonical immutable revision/head records prepared by Rust.
- Stored human-readable timestamps use local wall time with an explicit numeric UTC offset. Native imports recursively convert UTC RFC 3339 frontmatter timestamps to the system time zone while preserving the instant; browsers provide their local offset to Rust when preparing authoritative mutations. PWA presentation renders local timestamps without showing the stored offset.
- Cached records and pending writes survive offline reloads. A raw record explorer remains available for diagnostics.
- The endpoint secret, author secret, and writable ticket are AES-GCM encrypted in IndexedDB with a non-extractable WebCrypto key. Cached entries and pending writes are separate and are currently not encrypted.
- The service worker caches only the versioned application shell. The application checks uncached deployment metadata on load, cached restoration, reconnect, and every ten minutes before offering an **Update** button only when the deployed version is newer.
- Production nginx serves static assets only; there is no application API, sync gateway, writable server workspace, or server-side action executor.

Clearing browser storage destroys the browser identity and writable capability. The encrypted vault protects raw IndexedDB exports, not malicious same-origin JavaScript or XSS.

### `xo-syncd`

`xo-syncd` is an ordinary durable workspace peer, not an ordering authority.

- It provides an always-available Docs/Blobs replica and bootstrap participant.
- Public health and authenticated operator status, metrics, and setup endpoints support operations without exposing workspace capabilities.
- `xo-admin` handles invitations, ticket import, device retirement, namespace rotation, diagnostics, and verified backup/restore.

## Canonical records

Each workspace is one Iroh Docs namespace. Every endpoint has its own endpoint identity and Docs author identity. Writable invitations contain the namespace capability and peer addressing; read-only invitations cannot advance a head.

Current record keys include:

```text
note/<note-id>/revision/<revision-id>  -> canonical CBOR NoteRevision
note/<note-id>/head/<author-id>        -> canonical CBOR Head
asset/<asset-id>                       -> canonical CBOR AssetRecord
asset-blob/<asset-id>                  -> verified asset bytes
config/<path>/<revision-id>            -> canonical CBOR ConfigRevision
config-blob/<revision-id>              -> verified Steel configuration bytes
device/<endpoint-id>                   -> canonical CBOR DeviceRecord
workspace/descriptor                   -> canonical CBOR WorkspaceDescriptor
```

A `NoteRevision` contains schema, note ID, complete frontmatter and body, materialized path, HLC, author ID, predecessor IDs, and deletion state. Its revision ID is the BLAKE3 hash of canonical CBOR bytes. A commit stores the immutable revision before advancing the local author's head.

Resolution validates the revision graph, filters unacceptable records, orders candidate heads by HLC and revision identity, and retains non-ancestor heads as conflicts. Editing a conflicted note uses the winning and concurrent heads as predecessors, preserving history while producing a merged successor. Deletion and restoration are ordinary immutable revisions.

Configuration and asset metadata bind size and BLAKE3 identity to separately replicated bytes. Native clients materialize winning notes and configuration; watcher suppression prevents projection writes from becoming duplicate revisions.

## Rust components

### `xo-core`

Shared authoritative code includes:

- IDs, HLCs, domain schemas, canonical CBOR identities, Markdown/YAML codecs, canonical paths, required frontmatter, and conflict resolution;
- predicates, views, subviews, bounded queries, templates, declarative actions, capability checks, and Steel configuration parsing;
- native record repositories, Iroh persistence, local indexes, backup/restore, projection, watchers, encryption, and synchronization state.

Native-only filesystem, SQLite, watcher, encryption, and persistent-Iroh modules are gated behind the `native` feature. `xo-web` disables that feature and reuses the browser-compatible domain, behavior, Markdown, HLC, ID, resolution, and Steel modules.

### `crates/xo-web`

The Wasm facade:

- owns the relay-only in-memory browser Iroh endpoint, canonicalizes discovered relay hostnames for strict WebKit TLS validation, and never tries to synchronize a new local workspace with its own endpoint;
- validates signed record keys and values, configuration content identities, revision identities, and graphs;
- resolves workspace snapshots and executes queries;
- prepares canonical create/edit/delete/restore revision and head writes; and
- exposes coarse JSON DTOs through `wasm-bindgen` to the dedicated worker.

### Binaries

- `xo`: TUI and native workspace peer.
- `xo-lsp`: stdio editor companion with recursive projection loading, live Markdown/ID diagnostics, and wikilink/tag completion; it is currently read-only.
- `xo-syncd`: durable headless peer.
- `xo-admin`: administration, backup, restore, invitations, and diagnostics.

## Security and operations

- Iroh endpoint keys identify endpoints; Docs author keys identify writes; invitations distribute namespace capabilities.
- Browser setup tickets use URL fragments so static HTTP requests and normal server logs do not receive the capability.
- Native network host capabilities require HTTPS, public-address validation, pinned DNS, disabled proxies and redirects, restricted headers, and bounded time and response size.
- Device retirement records establish author cutoffs without erasing accepted history.
- Backups verify Docs metadata and Blob content before restoration.
- UTC release timestamp tags are embedded in every binary and the PWA; CI verifies the reported version and static `version.json`.

## Validation

Automated coverage includes:

- canonical record encoding, graph validation, concurrent edits, deletion/restoration, device retirement, namespace rotation, and interrupted Blob recovery;
- native projection round trips, watcher suppression, configuration parsing, capability isolation, and backup restore;
- browser Rust tests for note mutation plus configured views/subviews;
- Playwright tests for create/edit/delete/restore, view switching, offline cached recovery, encrypted ticket storage, deployment updates, and browser convergence through a real native `xo-syncd` peer; and
- Rustfmt, Clippy with warnings denied, workspace tests, TypeScript checks, npm audit, production PWA builds, and multi-architecture release/container CI.
