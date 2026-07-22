# Sync v2 migration handoff

## Current state

The server-side v2 operation protocol is present in `internal/syncsvc/v2.go`.
It uses an epoch, operation UUID receipts, HLC versions, a monotonically
increasing feed cursor, acknowledgements, tombstones, manual device retirement,
and conservative tombstone compaction. Signed TUI endpoints are registered at:

- `GET /api/sync/v2/bootstrap`
- `POST /api/sync/v2/push`
- `GET /api/sync/v2/pull?cursor=N`
- `POST /api/sync/v2/ack`

The existing SQLite item/config/asset tables are still maintained as the web
projection after a winning v2 operation is accepted. Browser v2 bootstrap/push
routes and IndexedDB device identity helpers are also present.

This working tree's uncommitted TUI change is `internal/tui/sync.go`. It now
uses `syncV2` instead of the legacy push/snapshot calls. Existing local outbox
rows are translated to deterministic v2 operation IDs derived from epoch,
client ID, and outbox row ID. Incoming items, workspace config, and assets are
applied to the Markdown workspace without producing a new outbox entry.

## Important caveats

- This is not yet a safe production cutover. The cache schema and outbox are
  still the legacy schema; v2 translation is an upgrade bridge, not the final
  durable v2 client implementation.
- `syncV2` currently calls bootstrap every reconciliation. Bootstrap should be
  used only for a missing/changed epoch; normal cycles must use the saved cursor
  and pull endpoint.
- HLC state is not persisted or advanced from observed remote versions. The
  bridge derives its timestamp from the legacy outbox creation time and uses
  the row ID as its logical component.
- Item deletion relies on the operation path. A tombstone with no usable local
  path needs lookup by item ID from the cache before deleting the projection.
- Config writes are applied but the TUI does not reload the workspace config in
  response to a remote v2 config change; wire this to the existing
  `configChanged` flow.
- Asset transfer still uses the existing signed asset byte endpoints. Verify
  hash/metadata consistency before treating this as atomic.
- The PWA has v2 types/device helpers and browser push route, but its active
  `web/src/sync.ts` runtime still uses the legacy app outbox protocol.
- No filesystem watcher, duplicate-ID diagnostic store, malformed-Markdown
  diagnostic UI, or stale-tombstone suppression has been implemented.
- No `xo sync seed --source ...` clean-epoch command exists yet.

## Recommended next steps

1. Add v2 cache migrations: operation UUID, target version, HLC state, epoch,
   cursor, expected-write hashes, diagnostics, and asset metadata. Migrate or
   explicitly retire legacy rows once.
2. Refactor TUI sync so bootstrap occurs only when the stored epoch is absent
   or differs. Pull in pages until caught up, apply records transactionally,
   then acknowledge the cursor. Implement a persisted HLC `Next`/`Observe` API.
3. Add an fsnotify-backed TUI watcher with debounce and remote-write
   suppression. It must ignore `.exo/`, turn renames into ID-preserving
   upserts, and surface duplicate/malformed Markdown rather than guessing.
4. Implement the seed command. It must validate one selected source workspace,
   clear/recreate only the server sync epoch with explicit destructive consent,
   seed items/config/assets, and output a validation report.
5. Replace the PWA runtime with the same v2 operation/outbox/cursor model;
   store pending asset blobs in IndexedDB and expose browser device renaming.
6. Add server UI routes for retirement and compaction, with an audit-friendly
   confirmation flow.
7. Add integration tests for concurrent HLC conflict resolution, watcher
   suppression, stale file non-resurrection, retries, compaction, and the full
   cutover/bootstrap flow.

## Verification

The focused v2 server tests passed earlier with:

```sh
env -u GOROOT GOCACHE=/private/tmp/exo-gocache go test ./internal/syncsvc -run '^TestV2'
```

The TUI package currently compiles with:

```sh
env -u GOROOT GOCACHE=/private/tmp/exo-gocache go test ./internal/tui -run '^$'
```

Tests using `httptest.NewServer` cannot bind a loopback port in this sandbox;
run the full suite in a normal development shell with `task test`.
