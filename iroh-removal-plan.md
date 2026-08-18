# Iroh removal and centralized synchronization plan

## Objective

Replace the decentralized Iroh transport with one `xo-syncd` server per workspace.
The server is the synchronization hub, HTTP API, and embedded PWA host. Native and
browser clients remain offline-first: each keeps a durable local Automerge replica,
accepts local writes without a network connection, and reconciles through the same
WebSocket endpoint when connectivity returns.

This is a greenfield transport migration. Existing Iroh state, invitations, and
membership state are not migrated. Users export Markdown before upgrading and import
it into a new `xo-syncd` workspace.

## Fixed decisions

- One workspace is hosted by each `xo-syncd` process.
- Both clients synchronize at `GET /api/sync` using a WebSocket upgrade.
- The PWA uses its own origin (`ws:` or `wss:` as appropriate).
- The TUI receives the server URL only from `--server`; no replicated server address
  and no invitation/ticket discovery are used.
- The reverse proxy authenticates PWA requests. `xo-syncd` adds no application-level
  authentication to PWA, API, static asset, or WebSocket routes.
- A directly connected TUI is trusted in the same way as a proxy-authorized browser.
- Membership identities, approvals, removals, signed changes, invitations, Pkarr,
  Gossip, relay selection, QUIC protocols, and Iroh endpoint identities are removed.
- Client IDs remain human-readable presence labels only; they are not security
  identities.
- `xo-syncd` embeds the production PWA assets in its binary.
- URL capture keeps the existing public-HTTP(S)-only SSRF and redirect protections.
- Deleted records continue to use xo's immutable revision/tombstone semantics.

## Target protocol

1. The client opens `/api/sync` and sends a bounded JSON `client_hello` containing
   the protocol version and its display client ID.
2. The server replies with a JSON `server_hello` containing the protocol version,
   workspace ID, and currently connected client IDs.
3. Subsequent binary WebSocket messages are opaque Automerge sync messages generated
   with an independent `automerge::sync::State` for that connection.
4. Either side generates another sync message after applying remote changes or making
   a local change. Messages are bounded and malformed frames close only that client.
5. The server broadcasts presence control messages when clients connect/disconnect.
6. Clients reconnect with bounded exponential backoff and jitter. Their local replica
   remains usable while disconnected.
7. A local mutation is acknowledged to the UI only after the local Automerge replica
   is durable. Server durability is acknowledged separately through sync status.

Full-document replacement is not the steady-state protocol. Initial synchronization
may transfer the complete Automerge history through normal Automerge sync messages.

## HTTP surface

- [x] Specify `GET /healthz` as an unauthenticated probe returning exactly `ok\n`.
- [ ] Add `GET /api/items/{id}` returning `{ frontmatter, body }`.
- [ ] Add `POST /api/items` with `{ url }`, safe URL capture, and
  `{ id, frontmatter, body }` response.
- [ ] Add `PATCH /api/items/{id}` with optional `frontmatter` and `body`; reject a
  mismatched frontmatter ID and preserve omitted values.
- [ ] Add `DELETE /api/items/{id}` using an immutable deleted revision.
- [ ] Use consistent JSON errors, body limits, content types, and status codes.
- [ ] Add conditional request/concurrency documentation; Automerge remains the source
  of truth when API and synchronized clients race.

## Phase 1: protocol and server foundation

- [x] Add transport-neutral centralized sync handshake/control types and size limits
  to `xo-core`.
- [x] Add Automerge sync-state methods to the durable record store.
- [x] Replace the multi-workspace daemon startup with one required durable workspace.
- [x] Implement `/api/sync` WebSocket upgrade and per-connection Automerge sync state.
- [x] Persist every accepted server change before acknowledging/broadcasting it.
- [x] Track connected client IDs for status and the TUI peer view.
- [ ] Make graceful shutdown close listeners, flush the workspace, and close sockets.
- [x] Remove the bearer-token operator server and obsolete setup/invitation endpoints;
  retain unauthenticated health and useful operational metrics only if they do not
  expose note content.

## Phase 2: native client

- [ ] Add `xo --server http[s]://host[:port]` and derive `ws[s]://.../api/sync`.
- [ ] Remove `--ticket`, workspace invitations, and pairing commands.
- [ ] Replace `IrohNode`/`IrohWorkspace` with a transport-neutral local Automerge
  workspace and WebSocket synchronization task.
- [ ] Keep native snapshot durability and the single-process state lock.
- [ ] Reconnect automatically and synchronize offline edits in both directions.
- [ ] Replace membership management with a connected-clients view.
- [ ] Keep `open_peers` and show client ID, connection status, and last observation;
  remove approve, reject, remove, and retire-membership actions.
- [ ] Update footer/status language from relay/peer terminology to server sync.
- [ ] Update `xo-admin` or remove commands made obsolete by the HTTP API.

## Phase 3: browser client

- [ ] Remove the browser Iroh endpoint, relay, Gossip, Pkarr, invitation, membership,
  and signed-change code from Rust/Wasm.
- [ ] Add a small Wasm-owned Automerge replica API that generates/applies sync messages.
- [ ] Open a same-origin WebSocket from the dedicated worker.
- [ ] Restore IndexedDB replica and cached notes before opening the socket.
- [ ] Keep offline create/edit/delete and pending-sync indicators.
- [ ] Replace create/join invitation onboarding with automatic connection to the
  server workspace after choosing a client ID.
- [ ] Remove membership controls and invitation storage from IndexedDB and the UI.
- [ ] Preserve service-worker caching, immediate cached rendering, and update flow.
- [ ] Test offline reload, offline writes, reconnect convergence, and conflict retention.

## Phase 4: embedded PWA and item API

- [ ] Make the PWA build reproducible before `xo-syncd` embedding.
- [ ] Embed hashed assets, `index.html`, manifest, icons, service worker, and installer.
- [ ] Serve SPA fallbacks without shadowing `/api/*` or `/healthz`.
- [ ] Set immutable caching for hashed assets and no-cache headers for HTML, manifest,
  service worker, and version metadata.
- [ ] Implement the item API through the same authoritative revision/head model used
  by synchronized clients.
- [ ] Reuse the existing URL-capture parser, response limits, Rustls provider setup,
  redirect validation, and private-network rejection.

## Phase 5: removal and cleanup

- [ ] Remove `iroh`, `iroh-gossip`, relay, Pkarr, and QUIC dependencies from every
  Cargo manifest and the lockfile.
- [ ] Delete Iroh transport modules, ALPN protocols, invitation codecs, Gossip topics,
  membership registry/identity/event code, and signed-change envelopes.
- [ ] Remove endpoint IDs and cryptographic membership fingerprints from persisted and
  presentation contracts.
- [ ] Remove Iroh-related configuration, environment variables, installer prompts,
  operator setup pages, tests, CI services, and documentation.
- [ ] Rename remaining generic concepts (`IrohWorkspace`, `IrohNode`, etc.) before the
  migration is considered complete.
- [ ] Update architecture documents to describe a centralized but offline-first system.
- [ ] Ensure `rg -i 'iroh|pkarr|gossip|relay|invitation|membership'` only finds explicit
  historical migration notes where intentionally retained.

## Testing gates

- [ ] Unit-test protocol versioning, malformed controls, and frame size bounds.
- [x] Deterministic local WebSocket test: server plus two native replicas converge.
- [ ] Three-client conflict test: two offline edits converge through a restarted server.
- [ ] Server restart test proves acknowledged data survives.
- [ ] Browser test proves cached notes render before WebSocket connection.
- [ ] Browser test proves an offline mutation synchronizes after reconnect.
- [ ] TUI and browser converge through the same `/api/sync` endpoint.
- [ ] API GET/PATCH/DELETE changes appear in connected and later-reconnected clients.
- [ ] URL import tests cover private IPs, redirects, body limits, invalid content types,
  and successful readable Markdown extraction.
- [ ] Static asset tests cover SPA fallback, cache headers, service worker, and offline
  reload.
- [ ] `/healthz` returns status 200, `text/plain`, and exactly `ok\n` while ready.
- [ ] Workspace tests and Clippy pass without ignored central-sync tests.

## Completion criteria

The migration is complete only when production binaries contain no Iroh transport,
both clients use `/api/sync`, `xo-syncd` serves the embedded offline-first PWA and item
API, all required deterministic tests pass, and required CI/deployment jobs are green.
