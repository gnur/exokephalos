# iOS App Explanation For exo Sync

This document is written for an LLM or engineer building an offline-first iOS app that syncs with an existing `exo serve` instance. The iOS app should feel similar to the exo TUI: local-first note browsing/editing, an explicit first-time sync setup, visible sync state, and an inspectable retry queue.

## Product Goal

Build an iOS client for exo notes that works fully offline and syncs opportunistically with the central `exo serve` web UI.

The app should:

- Store notes locally and allow browsing, viewing, creating, editing, and deleting while offline.
- Sync with `exo serve` using the existing signed sync endpoints.
- Require a one-time client approval in the web UI before syncing.
- Keep retrying pending sync work automatically after approval.
- Show sync health clearly, without blocking local use.
- Expose a sync outbox/history view so users can see pending, failed, and synced operations.

The app should not depend on the server for day-to-day editing. The server is a synchronization peer and central store, not a required runtime dependency.

## User Experience

### Main Notes Experience

The app should provide:

- A list of notes/items.
- A detail view that renders markdown.
- A raw or structured editor for frontmatter plus body.
- Create and delete flows.
- Search/filtering over local data.
- Basic metadata display: `id`, `type`, `title`, `tags`, `created`, and any custom frontmatter fields.

The iOS app does not need to implement every TUI feature immediately, but it should preserve exo's data model:

- Each item has YAML-like frontmatter represented locally as structured data.
- Each item has a markdown body.
- Item identity is the frontmatter `id`.
- Views are normally driven by root-level TOML config in exo, but a first iOS version may provide a general notes list plus filtering by `type` and `tags`.

### Sync Setup

The setup flow should be explicit:

1. User enters the server URL, for example `https://exo.example.com`.
2. App chooses or asks for a client ID, for example `iphone-erwin`.
3. App generates an ed25519 keypair and stores the private key securely in the iOS Keychain.
4. App calls `POST /api/sync/enroll` with the public key.
5. App shows `pending approval`.
6. User opens the exo web UI and approves the client in the `sync clients` tab.
7. App polls enrollment status every 5 seconds.
8. Once approved, app uploads local notes/config and starts continuous sync.

The user should not need to press a second "start sync" button after approval.

### Sync Status

Show a compact sync indicator, similar to the TUI footer:

- `not configured`
- `pending approval`
- `connected`
- `offline`
- `sync error`
- `syncing` only for active foreground operations, not as a flickering recurring state

The app should keep working in all states except where the user is explicitly trying to configure sync.

### Sync Outbox

Provide a sync outbox/history screen.

It should show:

- Operation ID/local row ID.
- Status: `pending`, `failed`, `synced`.
- Operation: `upsert_item`, `delete_item`, `upsert_config`, `delete_config`.
- Target item ID or config path.
- Attempt count.
- Last error.
- Created time.
- Last attempt time.

It should support:

- Filtering by status.
- Viewing operation details/payload.
- Retrying one failed operation.
- Retrying all failed operations.

The app should retry automatically, but the outbox helps users understand what is happening.

## Local Data Model

Use a local SQLite database. The iOS app should be offline-first and should not store notes only in memory.

Recommended local tables:

### `items`

Stores the local note state.

Fields:

- `id TEXT PRIMARY KEY`
- `path TEXT NOT NULL`
- `frontmatter_json TEXT NOT NULL`
- `body TEXT NOT NULL`
- `type TEXT NOT NULL`
- `tags_json TEXT NOT NULL`
- `created TEXT NOT NULL`
- `updated_at TEXT NOT NULL`
- `deleted_at TEXT NOT NULL DEFAULT ''`
- `content_hash TEXT NOT NULL`
- `server_revision INTEGER NOT NULL DEFAULT 0`

`path` should be a stable relative path matching exo's sync payload shape, for example `notes/2026/07/example.md`. The iOS app does not need to write actual markdown files, but it should preserve paths from the server and generate reasonable paths for new items.

### `outbox`

Stores local changes that must be pushed to the server.

Fields:

- `id INTEGER PRIMARY KEY AUTOINCREMENT`
- `op TEXT NOT NULL`
- `target_kind TEXT NOT NULL`
- `target_id TEXT NOT NULL`
- `path TEXT NOT NULL`
- `payload_json TEXT NOT NULL`
- `status TEXT NOT NULL`
- `attempts INTEGER NOT NULL DEFAULT 0`
- `last_error TEXT NOT NULL DEFAULT ''`
- `created_at TEXT NOT NULL`
- `last_attempt_at TEXT NOT NULL DEFAULT ''`

Use statuses:

- `pending`
- `failed`
- `synced`

### `sync_meta`

Stores sync state.

Fields:

- `key TEXT PRIMARY KEY`
- `value TEXT NOT NULL`

Useful keys:

- `server_url`
- `client_id`
- `enrollment_token`
- `sync_started`
- `sync_last_revision`

The private key should be stored in the iOS Keychain, not in SQLite.

## Local Change Behavior

Any local create/update/delete must:

1. Update local `items`.
2. Add an outbox entry.
3. Trigger a near-immediate sync attempt if the client is approved.

For create/update, enqueue:

```json
{
  "op": "upsert_item",
  "target_kind": "item",
  "id": "abc1234",
  "path": "notes/2026/07/example.md",
  "frontmatter": {
    "id": "abc1234",
    "type": "note",
    "title": "Example",
    "tags": []
  },
  "body": "Markdown body\n"
}
```

For delete, enqueue:

```json
{
  "op": "delete_item",
  "target_kind": "item",
  "id": "abc1234",
  "path": "notes/2026/07/example.md"
}
```

Do not block local edits on network availability.

## Sync Flow

### First-Time Enrollment

Endpoint:

```http
POST /api/sync/enroll
```

Unsigned request.

Request:

```json
{
  "client_id": "iphone-erwin",
  "label": "iphone-erwin",
  "public_key": "base64-ed25519-public-key"
}
```

Response:

```json
{
  "status": "pending",
  "enrollment_token": "token"
}
```

Store `enrollment_token` locally and poll status.

### Enrollment Status

Endpoint:

```http
GET /api/sync/enroll/status?client_id={client_id}&token={token}
```

Unsigned request.

Response while waiting:

```json
{"status":"pending"}
```

Response after approval:

```json
{"status":"approved"}
```

The app should poll every 5 seconds while pending. Once approved, set `sync_started=true`, enqueue a full local snapshot if needed, push the outbox, pull the server snapshot, and open the event stream.

### Push Changes

Endpoint:

```http
POST /api/sync/changes
```

Signed request.

Request:

```json
{
  "changes": [
    {
      "op": "upsert_item",
      "target_kind": "item",
      "id": "abc1234",
      "path": "notes/2026/07/example.md",
      "frontmatter": {
        "id": "abc1234",
        "type": "note",
        "title": "Example",
        "tags": []
      },
      "body": "Markdown body\n"
    }
  ]
}
```

Response:

```json
{"revision":12}
```

On success:

- Mark all pushed outbox rows as `synced`.
- Clear `last_error`.
- Record `last_attempt_at`.
- Optionally update `sync_last_revision` if the returned revision is newer.

On failure:

- Mark pushed rows as `failed`.
- Increment `attempts`.
- Store the error message.
- Retry later.

Batch size can be modest, for example 100 changes per request.

### Pull Snapshot

Endpoint:

```http
GET /api/sync/snapshot
```

Signed request.

Response:

```json
{
  "items": [
    {
      "op": "upsert_item",
      "target_kind": "item",
      "id": "abc1234",
      "path": "notes/2026/07/example.md",
      "frontmatter": {
        "id": "abc1234",
        "type": "note",
        "title": "Example",
        "tags": []
      },
      "body": "Markdown body\n"
    }
  ],
  "configs": [
    {
      "op": "upsert_config",
      "target_kind": "config",
      "path": "notes.toml",
      "content": "default_view = \"notes\"\n"
    }
  ]
}
```

Apply item changes to local SQLite without creating new outbox entries. This is important: remote updates should not echo back as local changes.

For iOS v1, config entries may be stored for future use without fully implementing TOML-driven views.

### Listen For Server Events

Endpoint:

```http
GET /api/sync/events?since_revision={revision}
```

Signed request. This is a server-sent events stream.

Event:

```text
event: change
data: {"revision":12,"target_kind":"item","target_id":"abc1234","op":"upsert_item","created_at":"2026-07-12T10:00:00Z"}
```

On event:

1. Store `sync_last_revision`.
2. Pull `/api/sync/snapshot`.
3. Update local UI from SQLite.
4. Reconnect if the stream closes.

The server also sends comment pings like:

```text
: ping
```

Ignore comment lines.

## Request Signing

Signed endpoints:

- `POST /api/sync/changes`
- `GET /api/sync/snapshot`
- `GET /api/sync/events?since_revision=...`

Headers:

| Header | Value |
| --- | --- |
| `X-Exo-Client-ID` | Client ID |
| `X-Exo-Timestamp` | Current UTC timestamp in RFC3339Nano format |
| `X-Exo-Nonce` | Unique random nonce |
| `X-Exo-Signature` | Base64 ed25519 signature |

The signed message is:

```text
METHOD
REQUEST_URI
TIMESTAMP
NONCE
SHA256_BODY_HEX
```

Details:

- `METHOD` is uppercase, for example `GET` or `POST`.
- `REQUEST_URI` includes path and query, for example `/api/sync/events?since_revision=12`.
- `TIMESTAMP` is the exact value sent in `X-Exo-Timestamp`.
- `NONCE` is the exact value sent in `X-Exo-Nonce`.
- `SHA256_BODY_HEX` is lowercase hexadecimal SHA-256 of the request body.
- For `GET`, hash the empty body.
- Sign the UTF-8 bytes of the joined message using the ed25519 private key.
- Base64 encode the signature.

The server rejects:

- Missing signature headers.
- Timestamps more than 5 minutes old or 5 minutes in the future.
- Reused nonces for the same client.
- Unknown clients.
- Pending or revoked clients.
- Invalid signatures.

## Continuous Sync Policy

After approval, the app should:

- Push pending/failed outbox entries every 5 seconds.
- Pull a snapshot after successful pushes.
- Keep an SSE connection open for server-side changes.
- Pull a snapshot when an SSE change event arrives.
- Retry SSE connection if it closes or fails.
- Trigger an immediate sync attempt after local edits.

Avoid UI flicker: do not change the visible status to `syncing` on every background retry if the previous state was already `connected`. Prefer showing `connected` unless the user starts a foreground sync action or an error/offline state occurs.

## Conflict Behavior

The current exo sync API is last-write-wins at the item level.

The iOS app should:

- Treat local edits as authoritative until pushed.
- Apply server snapshots when received.
- Avoid creating outbox entries while applying server snapshots.
- Consider warning users if a local pending edit is overwritten by a newer server snapshot, but v1 may accept last-write-wins behavior.

For v1, do not invent a separate conflict resolution protocol because the server does not expose one yet.

## ID And Path Generation

Each item must have a stable lowercase `id` in frontmatter.

The existing exo ID scheme is:

- Base32 encoded days since `1989-01-17`.
- Base32 random string of 4 characters.
- Prefix with zeroes if shorter than 7 characters.

If reproducing that exactly is inconvenient in iOS v1, use a lowercase unique ID compatible with existing routes and filenames. Avoid uppercase IDs.

Recommended generated path shape:

```text
notes/YYYY/MM/slug-title.md
```

The server accepts the path supplied in the sync payload.

## Minimum Viable iOS Scope

A practical first implementation should include:

- Local SQLite store.
- Keychain ed25519 private key storage.
- Server URL/client ID settings.
- `start sync` setup flow.
- Approval polling.
- Notes list.
- Note detail markdown rendering.
- Create/edit/delete note.
- Local outbox.
- Automatic 5-second retry loop after approval.
- Signed push/snapshot/events integration.

Defer these unless needed:

- Full TOML view parsing.
- CEL view filters.
- Custom yq actions.
- URL import.
- Reading stats.
- LSP/editor features.

## Important Compatibility Notes

- The server-side `exo serve` approval UI is at `/admin/sync/clients`.
- The iOS app should only use `/api/sync/*` endpoints for sync data.
- The browser endpoint `/api/events` is unsigned and intended for the web UI, not the iOS sync client.
- The app should preserve unknown frontmatter fields.
- The app should preserve server-provided paths.
- The app should be usable with no network connection after initial local data exists.
