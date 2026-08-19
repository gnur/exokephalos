# xo architecture

xo is an offline-first knowledge system. One Automerge document is authoritative
for each workspace. Canonical CBOR revisions, per-author heads, configuration,
assets, devices, and tombstones are stored as Automerge byte records. Immutable
revision history and concurrent conflicts are retained even when one revision is
selected as the visible winner.

## Central synchronization

Each `xo-syncd` process owns one durable server workspace. Native and browser
clients keep independent local Automerge replicas and synchronize through the
same `/api/sync` WebSocket endpoint. JSON is used only for the bounded versioned
hello and presence controls; binary frames contain opaque Automerge sync
messages with independent sync state per connection.

The server fsyncs accepted Automerge changes before they can be observed by
another client. Native clients also persist local changes before reporting local
success. Clients remain usable while disconnected and reconnect with bounded
backoff. Human-readable client IDs are presence labels, not security identities.
`xo-syncd` performs no authentication; browser deployments require an
authenticating HTTPS reverse proxy, while directly connected TUI clients are
trusted.

## Records, HTTP API, and conflicts

Notes retain immutable revisions and per-author heads. HLC ordering chooses a
deterministic visible revision while concurrent revisions remain explicit
conflicts. Deletion and restoration are revisions. Workspace Steel configuration
is replicated state rather than a projection file.

`xo-syncd` exposes `GET`, `PATCH`, and `DELETE /api/items/{id}` plus URL capture
through `POST /api/items`. API writes use the same typed record repository,
revision graph, heads, HLC, and Automerge document as synchronized clients. URL
capture resolves and pins public addresses, revalidates redirects, rejects
private or special networks, and bounds response sizes.

## Embedded PWA

Release builds package the tested Vite output directly into `xo-syncd`. The
server supplies `index.html`, hashed JavaScript/CSS/Wasm assets, manifest, icons,
service worker, version metadata, and installer. Hashed assets are immutable;
the application shell and update metadata are revalidated. Extensionless client
routes use the SPA fallback without shadowing `/api/*` or `/healthz`.

The browser is still being migrated from its transitional Iroh worker to a
Wasm-owned IndexedDB Automerge replica using `/api/sync`. Until that phase is
complete, browser invitation, membership, relay, and signed-change code remains
legacy code rather than part of the target architecture.

## Components

- `xo-core`: domain records, Automerge persistence, centralized sync contracts,
  projection, encryption, shared safe URL capture, and Steel behavior.
- `xo`: durable native replica, reconnecting WebSocket client, TUI, Markdown
  projection, import/export, capture, and plugins.
- `xo-syncd`: authoritative workspace, WebSocket synchronization, item API,
  health probe, and embedded PWA host.
- `xo-web`: transitional Rust/Wasm browser runtime and React PWA; its transport
  migration remains in progress.
- `xo-admin`: legacy offline import/backup tooling pending centralized cleanup.
- `xo-lsp`: stdio editor diagnostics and completion over a native projection.

Each mutable native state directory is protected by `.xo-workspace.lock` and is
single-process owned.

## Security boundaries

Steel executes in a fresh bounded VM. Plugins receive only explicitly granted
host capabilities and secrets. `xo-syncd` trusts requests that reach it, so the
reverse proxy is the browser authentication boundary. URL capture does not trust
DNS names or redirects to remain public. Passphrase-encrypted note ciphertext is
authenticated to the note identity and may synchronize without exposing its
plaintext.

## Testing

Commit CI runs formatting, Clippy, deterministic workspace/server tests, Wasm and
browser builds, browser offline UI tests, release-binary matrices, and the
`xo-syncd` container. Published binaries embed the exact PWA artifact produced by
the browser job. Release tags additionally run explicitly identified extensive
workspace tests. The remaining browser-central convergence and offline-reconnect
tests are tracked in `iroh-removal-plan.md`.
