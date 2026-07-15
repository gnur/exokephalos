# exo API Reference

This document describes the HTTP API exposed by `exo serve`.

In local mode, the web UI and JSON item API read/write markdown files under `EXO_DIR`. When `.exo/serve.toml` enables `[sync.server]`, `exo serve` is still the regular web UI, but notes and synced root-level workspace config are stored in SQLite. TUI clients keep local markdown files and use the signed sync API.

The browser web UI is password-protected. On first boot, `exo serve` prints a generated 20-character base32 password to stdout and stores only an Argon2id hash. Browser routes and most web JSON API routes require the login cookie. The signed `/api/sync/*` endpoints do not use the browser cookie; they authenticate TUI/iOS clients with ed25519 request signatures.

`GET /api/items/{id}` also accepts API keys created in the web settings screen. API keys are formatted as `exo_<base62 random>`, are stored hashed, require an app name, expiration date, and CEL filter, and can be sent as either:

```http
Authorization: Bearer exo_...
X-API-Key: exo_...
```

The item must match the key's CEL filter or the endpoint returns `404`. Example filters:

```cel
type == "secret" && "acceptance" in tags
type == "note"
```

## Web JSON API

Error responses use JSON:

```json
{"error":"message"}
```

### `GET /api/items/{id}`

Returns one item by frontmatter `id`.

Authentication: login cookie or API key. API-key requests are limited by the key's CEL filter.

Response:

```json
{
  "frontmatter": {
    "id": "apibook",
    "type": "book",
    "title": "API Book"
  },
  "body": "Book body\n"
}
```

### `PATCH /api/items/{id}`

Updates an item by frontmatter `id`.

Request body:

```json
{
  "frontmatter": {
    "id": "apibook",
    "type": "book",
    "title": "API Book"
  },
  "body": "Book body\n"
}
```

`frontmatter`, `body`, or both may be provided. Provided fields replace the complete stored value; omitted fields are preserved.

Response:

```json
{
  "frontmatter": {
    "id": "apibook",
    "type": "book",
    "title": "API Book"
  },
  "body": "Book body\n"
}
```

### `POST /api/items`

Creates a note from a URL.

Request body:

```json
{"url":"https://example.com/article"}
```

Response:

```json
{
  "id": "abc1234",
  "frontmatter": {
    "id": "abc1234",
    "type": "note",
    "title": "Example Article"
  },
  "body": "Imported markdown body\n"
}
```

This endpoint is available in local filesystem mode. In sync-enabled serve mode, URL import currently returns an error because the importer writes through the local markdown repository.

### `POST /api/query/ids`

Returns item IDs matching a CEL expression.

Request body is plain text using the same CEL environment as view filters: `type`, `tags`, and `fm`.

```cel
type == "book" && "reading" in tags
```

Response:

```json
{"ids":["apibook"]}
```

## API Key Management

These routes require the browser login cookie.

### `GET /api/app/api-keys`

Lists API key metadata. Raw keys and hashes are never returned.

### `POST /api/app/api-keys`

Creates an API key. The raw key is returned once.

Request:

```json
{
  "app_name": "Raycast",
  "expires_at": "2026-12-31",
  "filter": "type == \"note\""
}
```

`expires_at` must be in the future and no more than 1 year away.

### `POST /api/app/api-keys/{id}/revoke`

Revokes an API key while preserving its metadata for audit history.

## Web UI Routes

These routes render HTML:

| Route | Description |
| --- | --- |
| `GET /login` | Password-only login screen |
| `POST /login` | Create web session cookie |
| `GET /settings/password` | Password change form |
| `POST /settings/password` | Change web password |
| `GET /views/{viewId}` | List items; supports `?tags=a,b` and `?subview=Name` |
| `GET /views/{viewId}/stats` | Stats page when the view has `stats_template` |
| `GET /views/{viewId}/new` | New item form |
| `POST /views/{viewId}/new` | Create item |
| `GET /views/{viewId}/{itemId}` | Item detail |
| `GET /views/{viewId}/edit/{itemId}` | Raw markdown editor |
| `POST /views/{viewId}/edit/{itemId}` | Save raw markdown |
| `POST /views/{viewId}/delete/{itemId}` | Delete item |
| `POST /views/{viewId}/items/{itemId}/actions/{name}` | Execute configured action |
| `GET /import-url` | URL import form |
| `POST /import-url` | Import URL as note |
| `POST /webhook/{source}` | Store webhook payload as an item |

## Web SSE

### `GET /api/events`

Unsigned browser-only server-sent event stream used by the web UI in sync-enabled serve mode.

The stream starts at the current latest revision, so opening a page does not replay old history. Future revisions are sent as `change` events:

```text
event: change
data: {"revision":12,"target_kind":"item","target_id":"abc1234","op":"upsert_item","created_at":"2026-07-12T10:00:00Z"}
```

The web UI listens to this stream and refreshes the current page when notes or config change. It skips refresh while a form control is focused.

## TUI Sync API

The TUI uses these endpoints when `.exo/tui.toml` contains:

```toml
[sync]
server_url = "http://localhost:8293"
client_id = "laptop"
```

The initial enrollment request is unsigned. After approval, all sync data requests are signed with the client's generated ed25519 private key.

### `POST /api/sync/enroll`

Registers or refreshes a pending client enrollment.

Request body:

```json
{
  "client_id": "laptop",
  "label": "laptop",
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

Approve the client in the web UI at `/admin/sync/clients`. The TUI keeps polling after `start-sync`, so a second `start-sync` is not required after approval.

### `GET /api/sync/enroll/status?client_id={client_id}&token={token}`

Returns the enrollment status for the pending token.

Response:

```json
{"status":"pending"}
```

or:

```json
{"status":"approved"}
```

### `POST /api/sync/changes`

Signed. Pushes local outbox changes to the server.

Request body:

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

Supported item operations:

| Operation | Meaning |
| --- | --- |
| `upsert_item` | Create or update an item |
| `delete_item` | Mark an item deleted |
| `delete` | Delete alias accepted by the server |

Supported config operations:

| Operation | Meaning |
| --- | --- |
| `upsert_config` | Create or update a root-level workspace TOML config |
| `delete_config` | Mark a config deleted |
| `delete` | Delete alias accepted by the server |

Response:

```json
{"revision":12}
```

### `GET /api/sync/snapshot`

Signed. Returns the current server state for local reconciliation.

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
        "title": "Example"
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

### `GET /api/sync/events?since_revision={revision}`

Signed. Persistent server-sent event stream used by TUI clients.

Events are emitted for revisions newer than `since_revision`:

```text
event: change
data: {"revision":12,"target_kind":"item","target_id":"abc1234","op":"upsert_item","created_at":"2026-07-12T10:00:00Z"}
```

The TUI stores the latest revision locally and pulls a snapshot after receiving an event.

## Sync Request Signing

Signed sync endpoints require these headers:

| Header | Description |
| --- | --- |
| `X-Exo-Client-ID` | Client ID from `.exo/tui.toml` or hostname fallback |
| `X-Exo-Timestamp` | RFC3339Nano UTC timestamp; must be within 5 minutes |
| `X-Exo-Nonce` | Unique nonce for this client |
| `X-Exo-Signature` | Base64 ed25519 signature |

The signed message is:

```text
METHOD
REQUEST_URI
TIMESTAMP
NONCE
SHA256_BODY_HEX
```

`REQUEST_URI` includes the path and query string, for example `/api/sync/events?since_revision=12`. For `GET` requests, the body hash is the SHA-256 of an empty body. The server rejects stale timestamps, reused nonces, unknown clients, revoked clients, pending clients, and invalid signatures.

## Client Approval Routes

These routes are rendered inside the regular web UI when sync storage is enabled:

| Route | Description |
| --- | --- |
| `GET /admin/sync/clients` | List enrolled clients; auto-refreshes |
| `POST /admin/sync/clients/{clientId}/approve` | Approve a pending/revoked client |
| `POST /admin/sync/clients/{clientId}/revoke` | Revoke a client |

The approve button is hidden after a client is approved. Revoked clients must enroll or be approved again before signed sync requests are accepted.
