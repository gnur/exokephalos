# Image assets handoff

## Goal

Support workspace image assets in the web UI, TUI, and signed sync protocol.
Assets are referenced from notes with standard Markdown paths such as
`![photo](assets/photo.jpg)`.

## Implemented so far

- `internal/assets` provides shared asset import and path validation.
  - Stores files below the workspace root in `assets/`.
  - Accepts JPEG, PNG, GIF, and WebP, with a 20 MiB limit.
  - Sanitizes original filenames and adds a short hash suffix on conflicting
    content.
  - Returns the relative path, MIME type, SHA-256 hash, and size.
- The web server exposes:
  - `POST /api/app/assets` for multipart image uploads.
  - `GET /assets/{path...}` for serving workspace assets.
- The React raw editor supports upload-button, drag-and-drop, and pasted image
  files. A successful upload inserts a Markdown image reference.
- `npm run build:web` and the affected Go handler tests passed when this work
  was added.

## Important current limitations

- Uploaded assets are not tracked in cache metadata or the sync outbox.
- The sync server has no asset schema, metadata snapshot, or binary transfer
  endpoints.
- The TUI cannot attach images and does not render them through Kitty graphics.
- Review the auth middleware before shipping: `/assets/` must require the
  normal authenticated application session rather than being treated as an
  anonymous static path.
- The web editor currently appends image Markdown; improve it to insert at the
  textarea cursor and show upload errors inline.

## Completion plan

### Asset metadata and sync

- Add `assets` tables to local cache and sync-server SQLite databases with
  path, SHA-256 hash, MIME type, size, revision, timestamps, and tombstone
  state.
- Extend revisions/events and sync snapshots with `target_kind: "asset"` and
  asset metadata only. Do not include binary data in normal JSON changes.
- Scan root `assets/` files during local sync and enqueue asset
  create/update/delete metadata changes.
- Add signed streaming endpoints:
  - `PUT /api/sync/assets/{path...}` for validated upload.
  - `GET /api/sync/assets/{path...}` for download.
- Upload the binary before marking its outbox entry synced. During snapshot
  reconciliation, compare hashes and fetch only missing/changed files; apply
  tombstones locally.
- Keep filename collision behavior from `internal/assets` and store server
  assets under the sync server workspace's `assets/` directory.

### Web completion and security

- Record uploaded asset metadata and enqueue sync work in `AppAssetUpload`.
- Restrict asset serving to validated workspace paths and authenticated users;
  send image MIME type and `X-Content-Type-Options: nosniff`.
- Retain picker, drag/drop, and paste uploads, but insert returned Markdown at
  the current cursor position and present upload errors in the editor.

### TUI attachment and preview

- Add an `attach-image` action for the selected note. Prompt for a local path,
  import it through `assets.Import`, insert a Markdown reference into the note,
  notify the cache, and enqueue the asset for sync.
- Parse local `assets/...` Markdown image references in the selected note's
  preview.
- Render the first visible supported image via
  `github.com/charmbracelet/x/ansi/kitty` using file transmission and bounded
  preview-pane rows/columns.
- Assign stable image/placement IDs and clear old placements on selection,
  scroll, resize, content updates, and shutdown.
- For non-Kitty terminals, missing files, or unsupported graphics, retain the
  normal text preview and image Markdown reference.

## Verification

- Unit-test import validation, collision naming, hash calculation, and path
  traversal rejection.
- Test signed asset upload/download, metadata snapshots, reconciliation, and
  tombstones between two workspaces.
- Test authenticated web serving and web upload failure/success contracts.
- Test TUI attachment, Kitty command generation/cleanup, preview dimensions,
  and text fallback without requiring a graphical terminal.
- Run `go test ./...` and `npm run build:web`.
