# xo

**xo is an offline-first knowledge workspace backed by Automerge.** Each client
keeps a durable local replica and can create or edit notes without connectivity.
All replicas synchronize through one `xo-syncd` server per workspace over the
shared `/api/sync` WebSocket endpoint.

The project has three clients:

- **`xo`** is the terminal workspace and Markdown projection. Connect it with
  `xo --server https://notes.example.test` (the default is
  `http://127.0.0.1:9464`).
- **`xo-web`** is an installable offline-first PWA. Its worker owns a durable
  Automerge replica and synchronizes with the same-origin `/api/sync` endpoint.
- **`xo-syncd`** is the authoritative durable synchronization server. It hosts one
  workspace and will also serve the embedded PWA and item HTTP API.

Local writes are durable before the UI acknowledges them. When disconnected,
clients retain cached notes and pending Automerge changes. Reconnection preserves
immutable revision history and concurrent conflicts rather than overwriting one
client with another.

`xo-syncd` deliberately has no application authentication. Deploy it behind an
authenticating reverse proxy for browser access. A TUI may connect directly on a
trusted network. Client IDs are presence labels, not security identities.

Start a server and TUI with:

```console
xo-syncd --state-dir /var/lib/xo-syncd --bind 127.0.0.1:9464
xo --server http://127.0.0.1:9464
```

The Markdown directory remains a projection rather than the synchronization
transport or a complete backup. Keep the local state directory and server state
in normal backup procedures.

`xo-syncd` also exposes an unauthenticated item API on the same trusted or
reverse-proxy-protected origin:

- `GET /api/items/{id}` returns `frontmatter` and `body`.
- `POST /api/items` accepts `{ "url": "https://…" }` and safely captures a public
  HTML page as a new item.
- `PATCH /api/items/{id}` accepts optional `frontmatter` and `body` fields.
- `DELETE /api/items/{id}` creates an immutable deleted revision.

JSON request bodies are limited to 1 MiB. URL capture independently limits
responses, validates every redirect, and rejects private or special addresses.

The centralized transport migration is intentionally breaking. Existing legacy
workspaces move through Markdown export/import rather than in-place state migration.
See [`iroh-removal-plan.md`](iroh-removal-plan.md) for the completed migration.

## Example workflows

### 1. Private offline notebook

Connect `xo` to the workspace server once, then create and edit notes in
`~/notes` with or without connectivity. Each save creates an immutable local
revision; pending changes synchronize automatically when `xo-syncd` is reachable.

### 2. Browser and phone companion

Open the PWA served by the proxy in front of `xo-syncd`. The browser keeps its
Automerge replica in IndexedDB, renders cached notes immediately, and continues
creating and editing while offline. It reconnects to the same-origin
`/api/sync` endpoint when connectivity returns.

### 3. Always-on home or server

Run `xo-syncd` on a workstation, NAS, VPS, or small home server and put an
authenticating HTTPS reverse proxy in front of it. TUI clients use `--server`;
browsers use the same origin. No invitation or membership setup is required.

### 4. Multi-device offline collaboration

Give each device its own state directory and client ID. Two clients can edit
while disconnected. xo keeps concurrent revisions and deterministically selects
a visible winner without deleting the other branch. Reconnect both through the
server, then edit the desired content to create a descendant of all branches.

### 5. Structured knowledge workflows

Configure replicated views and subviews for notes, books, projects, or reading
queues. Use tags and predicates to build filtered panes, then invoke declarative
actions such as adding tags, changing fields, or appending body content. A
capability-gated URL capture action and the sandboxed Hardcover plugin can turn
external information into ordinary replicated notes.

### 6. Markdown and editor workflow

Use `xo import` to bring in a Markdown tree, work in the TUI or an editor, and
use `xo export` when a conventional Markdown handoff is needed. `xo-lsp`
provides frontmatter diagnostics and completion for `[[note-links]]` and tags.
The projection stays human-readable while the replicated record graph preserves
identity, history, and conflicts.

This project is under active development. The commands below describe the
currently implemented workflow.

## Build the binaries

The workspace requires Rust 1.89 or newer.

```console
cargo build --release -p xo -p xo-lsp -p xo-syncd
```

The resulting programs are:

- `target/release/xo` — the terminal UI and import/export client
- `target/release/xo-lsp` — editor diagnostics and completion
- `target/release/xo-syncd` — the centralized synchronization and web server

The examples below assume those binaries have been copied somewhere in
`PATH`.

GitHub Actions builds release archives for Linux x86-64, Linux ARM64, macOS
ARM64, and Windows x86-64. Pushing a UTC timestamp tag creates the corresponding
GitHub Release automatically with generated release notes, all four archives,
the embedded-PWA binaries, and a `SHA256SUMS` file. The exact tag is embedded as the version
reported by every binary and by the PWA.

Create a release tag from a clean, fully committed checkout with:

```console
./release.sh
```

The script creates an annotated UTC tag in ISO 8601 basic format, such as
`20260728T143012Z`, then asks whether it should push the tag to `origin`. The
default answer is no. It refuses to tag while staged, unstaged, or untracked
changes exist. Pushing the tag starts the GitHub Release workflow.

## Quick Install (`xo` or `xo-syncd`)

You can install the native `xo` TUI, `xo-syncd` background daemon, and `xo-lsp`
from the repository-root installer:

```console
curl -fsSL https://raw.githubusercontent.com/gnur/exokephalos/main/install.sh -o install.sh
bash install.sh
```

After you self-host `xo-syncd`, the same script is also available from that
server's `/install.sh` route. No separate public static deployment is required.

The installer detects your OS and CPU architecture (Linux x86-64/ARM64 or macOS Apple Silicon), fetches the latest release archive from GitHub, extracts the binaries to `~/.local/bin`, generates `~/.config/xo/config.scm` with `xo config-init`, and prompts you to configure `xo` and/or `xo-syncd`. The TUI uses `~/.local/share/xo`; the systemd user daemon uses the separate `~/.local/share/xo-syncd` state directory.

When systemd setup is selected, the installer creates a user service for the
single centralized workspace. No ticket, invitation, membership approval, or
pairing step is required. Point native clients at that daemon with `xo --server`.
For browser access, expose the same daemon through an authenticating HTTPS
reverse proxy.

## Run the xo-web PWA

`xo-web` is a mobile-first React application with a Rust/Wasm runtime in a
dedicated worker. It provides URL-backed navigation, views and subviews,
search/tag filtering, rendered Markdown, editing, conflict history, and an
offline application shell.

The worker owns a Wasm Automerge replica, restores it from IndexedDB before
networking, and connects automatically to same-origin `/api/sync` after the user
chooses a presence client ID. Local writes persist the complete replica before
the UI reports success and synchronize after reconnect. Browser invitations,
membership identities, relay, Gossip, Pkarr, and signed-change state have been
removed.

Production PWA assets are embedded in `xo-syncd`; there is no separate static
production deployment. Open the authenticated origin serving the daemon. Static
assets and cached UI remain client-side, while synchronization and the item API
share that origin. The development fallback page is not the production PWA.

Build the Wasm package and static application locally with:

```console
cargo install wasm-pack --version 0.13.1 --locked
cd web
npm ci
npm run build:wasm
npm run build
```

### Embed xo-web in xo-syncd

Release builds package `web/dist` directly into `xo-syncd`. The daemon serves
the application shell, service worker, manifest, installer, icons, Wasm, and
hashed assets from the same origin as `/api/sync` and `/api/items`. Client-side
routes receive the embedded `index.html`; `/api/*` and `/healthz` are never
shadowed by the SPA fallback.

For a local production build, build the PWA first and provide its directory when
compiling the daemon:

```console
XO_PWA_DIR="$PWD/web/dist" cargo build --release -p xo-syncd
```

Without `XO_PWA_DIR`, development builds contain a small diagnostic fallback
page. Published binaries and the `xo-syncd` container always use the tested PWA
artifact. Put the daemon behind an authenticating HTTPS reverse proxy for
browser access.

## How synchronization works

Every client has a durable local Automerge replica. A save is acknowledged after
that replica is persisted, regardless of network availability. The reconnecting
client exchanges opaque Automerge sync messages with `xo-syncd` at `/api/sync`.
The daemon durably applies accepted changes before notifying other connections.

Canonical CBOR note revisions and per-author heads live inside Automerge. HLC
ordering chooses a visible winner without discarding concurrent branches.
Deleting an item creates a deleted revision; editing a conflicted item creates a
descendant of all retained branches. The Markdown directory is a native
projection, not the synchronization protocol.

The first subview is selected by default. Views, subviews, predicates, sort
fields, actions, templates, and capability grants are replicated workspace
configuration.

## Start a centralized workspace

The following is the complete laptop-plus-mobile setup flow. It assumes the
laptop will run the authoritative server and that a DNS name such as
`notes.example.com` can point to the laptop or home-server network.

### 1. Install the binaries

On the laptop, install a release archive or build the three required binaries:

```console
cargo build --release -p xo -p xo-lsp -p xo-syncd
```

Copy `xo`, `xo-syncd`, and optionally `xo-lsp` into `PATH`. There is no
`xo-admin` binary.

### 2. Start the server locally

Create a directory that will be backed up separately from the Markdown
projection, then start one server workspace:

```console
mkdir -p ~/.local/share/xo-syncd
xo-syncd --state-dir ~/.local/share/xo-syncd --bind 127.0.0.1:9464
```

Keep this process running, or use the systemd user service created by the
installer. Verify it locally:

```console
curl -fsS http://127.0.0.1:9464/healthz
# ok
```

The first server start creates the workspace. One `xo-syncd` process hosts one
workspace; do not point two independent server state directories at the same
workspace.

### 3. Prepare and import existing Markdown

Initialize the native client configuration and choose a projection directory:

```console
mkdir -p ~/.config/xo
xo config-init > ~/.config/xo/config.scm
# Edit config.scm if you want a projection other than ~/notes.
```

The Markdown tree is a projection, not the authoritative store. For an existing
folder, make a copy outside the configured projection and import the copy into
the fresh server workspace:

```console
mv ~/notes ~/notes-before-xo
mkdir -p ~/notes
cp -a ~/notes-before-xo ~/xo-import
xo --server http://127.0.0.1:9464 import ~/xo-import --type note
```

`xo import` validates the source before writing, leaves the source untouched,
commits the items to `xo-syncd`, and materializes the authoritative notes into
`~/notes`. If the source contains books or another item type, use the matching
`--type` value. Do not import the active projection itself, and do not run two
`xo` processes against one client state directory.

After import, start the TUI against the same server:

```console
xo --server http://127.0.0.1:9464 --client-id laptop
```

The laptop's local Automerge replica is durable and can continue operating when
the server is temporarily unavailable. Its state directory is separate from
the server directory and should also be backed up.

### 4. Publish the server for mobile access

Do not expose unauthenticated `xo-syncd` directly to the Internet. Put an
HTTPS reverse proxy with authentication in front of it, preserve WebSocket
upgrades, and proxy the same origin to `127.0.0.1:9464`. For example, the
proxy must route:

```text
https://notes.example.com/          -> http://127.0.0.1:9464/
https://notes.example.com/api/sync  -> WebSocket http://127.0.0.1:9464/api/sync
https://notes.example.com/api/...   -> http://127.0.0.1:9464/api/...
```

Configure DNS and HTTPS certificates for `notes.example.com`, and require the
same authentication policy for the PWA, item API, and WebSocket endpoint. A
reverse proxy is also the TLS termination point; `xo-syncd` intentionally does
not implement authentication or TLS.

For a LAN-only setup, a trusted HTTP reverse proxy or direct LAN binding can be
used instead, but it provides no application authentication and should not be
Internet-facing.

### 5. Open the PWA on the phone

On the phone, open `https://notes.example.com/`, complete the proxy
authentication, choose a human-readable client ID such as `phone`, and connect.
The PWA restores its IndexedDB replica before connecting, so it remains usable
offline after its first successful load. Install it to the home screen if
desired. The phone and laptop then synchronize through the same `/api/sync`
endpoint; no invitation, ticket, pairing, or separate mobile server is needed.

### 6. Back up and operate it

Back up both `~/.local/share/xo-syncd` and the laptop's `~/.local/share/xo`
state directory with their processes stopped. Markdown export is the portable
handoff format:

```console
xo --server http://127.0.0.1:9464 export ~/xo-export
```

For additional native clients, use a separate state and projection directory
and point each client at the same server. `--server` is the only native
server-discovery mechanism. `open_peers` shows currently connected client IDs;
these are presence labels and do not grant or revoke access.

### Server routes

- `GET /healthz` returns exactly `ok\n`.
- `GET /api/sync` upgrades to centralized Automerge synchronization.
- `GET /api/items/{id}` returns an item.
- `POST /api/items` safely captures a public URL.
- `PATCH /api/items/{id}` updates supplied fields through a new revision.
- `DELETE /api/items/{id}` creates a deleted revision.
- Other GET routes serve the embedded PWA with SPA fallback.

The item API currently has no ETag or conditional-write contract. API mutations
are serialized inside `xo-syncd`, while races with synchronized clients are
resolved by the Automerge revision graph: concurrent heads are retained and HLC
ordering selects the visible winner. Callers that require compare-and-swap must
first coordinate outside the API; a future protocol version may add explicit
preconditions.

### systemd

The installer can create a systemd user service. A direct unit is equivalent to:

```ini
[Unit]
Description=xo synchronization server
After=network-online.target

[Service]
ExecStart=%h/.local/bin/xo-syncd --state-dir %h/.local/share/xo-syncd --bind 127.0.0.1:9464
Restart=on-failure

[Install]
WantedBy=default.target
```

### Container

The supported image is the combined daemon, API, and embedded PWA:

```console
docker run --rm -p 9464:9464 -v xo-data:/data \
  ghcr.io/gnur/xo-syncd:latest
```

The image health check uses `/healthz`. Published multi-platform images reuse
release binaries containing the same tested PWA artifact as the binary release.

### TUI actions, key bindings, navigation, and tag filtering

Every normal-mode interaction is a named action. Press `:` to open the
autocompleting action picker, type part of an action name, use Up/Down to select,
Tab to complete, and Enter to run it. Short aliases are accepted where listed;
for example, `:q` runs `quit`, while `:p` opens the connected-client view.

| Action | Alias | Arguments | Effect |
| --- | --- | --- | --- |
| `action_picker` | — | — | Open the action picker. |
| `clear_search` | — | — | Clear the title search and return to normal mode. |
| `create_encrypted_item` | — | — | Create an encrypted note. |
| `create_item` | `c` | — | Create a plaintext note. |
| `cursor_down` | `j` | — | Move the selection down. |
| `cursor_up` | `k` | — | Move the selection up. |
| `delete_item` | `d` | — | Delete the selected note. |
| `edit_item` | `e` | — | Edit the selected note. |
| `edit_workspace_config` | — | — | Edit replicated workspace configuration. |
| `focus_column_left` | `h` | — | Focus the column to the left. |
| `focus_column_right` | `l` | — | Focus the column to the right. |
| `focus_subview_next` | — | — | Select the next subview. |
| `focus_subview_previous` | — | — | Select the previous subview. |
| `goto_view` | — | `view[/subview]` (required) | Open a view and optional subview. |
| `open_conflicts` | — | — | Show unresolved conflicts. |
| `open_goto` | — | — | Open the view/subview path prompt. |
| `open_item_actions` | — | — | Show configured actions for the selected item. |
| `open_peers` | `p` | — | Show clients currently connected to xo-syncd. |
| `open_search` | — | — | Edit the title search filter. |
| `open_sync_status` | — | — | Show synchronization status. |
| `open_view_picker` | `g` | — | Open the top-level view picker. |
| `quit` | `q` | — | Exit xo. |
| `refresh_sync` | — | — | Refresh and retry synchronization. |
| `restore_item` | `u` | — | Restore the most recently deleted note. |
| `retry_operation` | — | — | Retry the first failed synchronization operation. |
| `reverse_sort` | — | — | Reverse the current note sort order. |
| `toggle_selection` | — | — | Add or remove the highlighted note from the native multi-selection. |
| `toggle_tag` | — | — | Toggle the highlighted tag filter. |
| `toggle_tags_column` | — | — | Show or hide the tag column. |
| `unlock_preview` | — | — | Unlock the selected encrypted note preview. |

The TUI creates `~/.config/xo/keys.scm` on first start and hot reloads it while
running. You can also write the default explicitly:

```console
xo keymap-init > ~/.config/xo/keys.scm
```

Bind keys to actions with declarative forms:

```scheme
(keys
  (bind "j" cursor_down)
  (bind "h" focus_column_left)
  (bind "tab" focus_subview_next)
  (bind "e" edit_item)
  (bind "/" open_search)
  (bind "esc" clear_search)
  (bind "d" delete_item)
  (bind "g" open_view_picker)
  (bind ":" action_picker)
  (bind "b" (goto_view "books/read"))
  (bind "q" q))
```

The argument-bearing binding above opens the `books` view with its `read`
subview. The same action can be run without a binding by entering
`:goto_view books/read` in the action picker.

Names such as `space`, `enter`, `tab`, `backtab`, `left`, `right`, `up`, and
`down` represent special keys; modifier forms such as `ctrl+x` are also
accepted. The footer is generated from the active bindings and changes after a
successful hot reload. Invalid edits leave the previous keymap active and show a reload error.

By default `g` opens the top-level view switcher, Tab and Shift-Tab cycle
subviews, and Left/Right or `h`/`l` move between Tags, Notes, and Preview.
Highlight a tag with Up/Down or `j`/`k`; Space toggles that tag and does nothing
outside the tag column.

Tag counts are live facets. They first respect the active view or subview and
the `/` title query, then show how many notes would remain if each tag were
added to the currently selected tag filters. Selecting or clearing a tag
therefore updates every displayed count immediately. Escape runs the default
`clear_search` binding, clearing the title filter even after the search prompt
has been closed.

### Markdown projection layout

Projected notes use one canonical path derived from their ID and title:

```text
<first-three-ID-characters>/<ID>-<title-slug>.md
```

For example, note `01KABC123` titled “Server setup” is stored as
`01K/01KABC123-server-setup.md`. Editing the title moves the projected file to
its new canonical path. The replicated note ID remains stable, and filesystem
moves to other paths are reconciled back to the canonical layout.

## Import and export Markdown

`xo import` recursively imports Markdown into the configured active workspace:

```console
xo import ~/incoming-notes
xo import ~/incoming-books --type book
```

The source must be a directory outside the active Markdown projection. xo scans
and parses every Markdown file before committing the first revision, reports
all malformed-document and duplicate-ID diagnostics, and rejects IDs or paths
that collide with the active workspace. Files without complete frontmatter get
stable IDs and the required `id`, `created`, `tags`, `title`, and `type` fields.
Generated timestamps use the system's local wall time with an explicit numeric
UTC offset. During import, RFC 3339 UTC timestamps anywhere in frontmatter are
converted to the equivalent instant in the system time zone, including the
historically correct daylight-saving offset. The source tree is never modified.
The command reports the number of discovered items, updates an in-place
`current/total` counter on terminals, and does not report completion until the
projection, durable Automerge replica, and local index have been finalized and closed.

`xo export` writes winning workspace notes as conventional Markdown:

```console
xo export ~/xo-export
xo export ~/xo-notes-export --type note
```

The destination must be new or empty; xo will not overwrite an existing file.
Output is grouped below `<type>/<year>/<month>/`, internal `id`, `type`, and
`created` fields are removed, and duplicate title slugs receive deterministic
`-1`, `-2`, and later suffixes. Encrypted note bodies retain their `id` because
their ciphertext is bound to it.

## Editor integration

`xo-lsp` is a stdio Language Server for projected Markdown workspaces. Editors
normally launch `xo-lsp` and provide a workspace folder during initialization;
`xo-lsp --workspace ~/notes` is available for clients that do not provide one.
The current server recursively indexes Markdown outside hidden directories,
publishes diagnostics for malformed frontmatter and missing, invalid, or
duplicate IDs, tracks unsaved full-document changes, and completes note IDs
inside `[[...]]` plus tags from the workspace. It does not mutate files yet.

## Edit workspace behavior

Workspace behavior is replicated state and is edited from the TUI. It is no
longer exposed as `xo.scm` inside the Markdown notes projection. Run `edit_workspace_config` from the `:` action picker to open the current
workspace configuration in `$EDITOR`. Save and exit to validate it and commit a new replicated configuration
revision. Other clients receive the configuration through `/api/sync`; use the
TUI refresh command after a remote configuration update.

The configuration uses native declarative Steel similar to:

```scheme
(workspace-config
  (schema 1)
  (default-view "notes")
  (query-limit 500)
  (views
    (view
      (id "notes")
      (name "Notes")
      (key "n")
      (show-tags #t)
      (title-field "title")
      (subtitle-field #f)
      (sort-field "created")
      (descending #t)
      (preview #f)
      (predicate (field-equals "type" "note"))
      (subviews)))
  (actions
    (action
      (id "capture-url")
      (description "Capture readable content from a URL")
      (predicate (always))
      (effects)
      (plugin (capture-url))))
  (templates)
  (capability-grants
    (grant
      (action "capture-url")
      (capabilities create-note network))))
```

Each view and subview may choose any frontmatter field with `(sort-field
"field-name")`; a subview without one inherits its parent view's field. Views
can reverse their ordering with `(descending #t)`. When neither level specifies
`sort-field`, it defaults to `created`. The TUI and PWA insert year headers from
the leading ISO
year in the selected sort field; missing or non-date values appear under **No
year**.

Predicates support `always`, `field-equals`, `has-tag`, `not`, `all`, and
`any`. Actions use declarative effects such as `add-tag`, `remove-tag`,
`set-field`, and `append-body`; `(set-field "started" (now))` stores the
host-supplied RFC 3339 execution timestamp in local wall time with an explicit
numeric UTC offset, without exposing an ambient clock.
Mutating actions require an explicit `mutate-note` capability grant. Optional lexical modules below
`modules/**/*.scm` use the same fields inside `(workspace-module ...)`.
Executable Forge plugins are local `~/.config/xo/plugins/*.scm` files; their
`xo-plugin-manifest` function contributes actions and their action entrypoint
runs in a fresh, time-bounded `Engine::new_sandboxed()` VM.

The `capture-url` plugin is a capability-gated native host action. Run `open_item_actions` from the `:` action picker,
select **Capture readable content from a URL**, and enter an HTTP or HTTPS URL.
The host validates public destinations and redirects, limits the response,
extracts the readable article, converts it to Markdown, and commits an ordinary
replicated note. Both `create-note` and `network` grants are required. Steel
itself receives no ambient network API.

### Steel Forge plugins

Executable plugins are local `xo` configuration, not replicated workspace
state. Steel Forge installs them below:

```text
~/.config/xo/plugins/*.scm
```

`xo` discovers those files when it starts. There is no `xo plugin` subcommand;
plugins are never copied into `xo-syncd` or synchronized to other clients. The
plugin directory must be installed separately on each client that should expose
the action.

Plugins use capability-checked host primitives:

```text
(xo-selected-item-ids)       ; JSON array of selected note IDs
(xo-note-content id)          ; JSON frontmatter/body object
(xo-all-tags)                 ; JSON array of workspace tags
(xo-http-get url headers)     ; bounded public HTTPS GET
(xo-http-post-json url headers body)
(xo-update-items operations)  ; queue immutable note revisions
(xo-create-item operation)    ; queue a new note
```

The existing prompt and choice protocol is also reusable: the host supplies the
input prompt and presents the plugin's returned `choices`; selected choices
become ordinary notes. The host owns persistence and the TUI; Steel cannot access
the filesystem, process, socket, or terminal directly. Rust/dylib extensions are
an explicit future Forge integration and must use the same host primitive boundary.

The bundled `plugins/manage-tags.scm` demonstrates the interactive tag-manager
primitive. Install it with Steel Forge, select notes in the TUI using
`toggle_selection` (or bind another key), then run `:manage-tags`. `xo` opens a
native tag popup and applies the chosen tags to all selected notes.

The Hardcover plugin follows the same model: Steel performs its GraphQL request
and normalization, while `xo` provides the input prompt, choice picker, and note
creation host primitives. Its required capabilities are `create-note`, `network`,
and `read-secret`; HTTP remains HTTPS-only, proxy-free, DNS-pinned to validated
public addresses, redirect-free, time-bounded, header-restricted, and limited
to 2 MiB.

`example-config.scm` is a complete multi-view example with nested predicates,
subviews, sorting, explicit mutation grants, and reading actions that set
`started` and `finished` to their host-supplied execution timestamp.

The declarative workspace/module form remains restricted. Configuration is parsed through a
strict boundary: arbitrary filesystem, environment, process, network, clock,
or evaluation expressions are rejected. Workspace modules remain replicated
below `modules/**/*.scm`; Forge plugins are local below
`~/.config/xo/plugins/`. The main workspace configuration is kept in replicated
state and edited with the `edit_workspace_config` action.

## Operations and recovery

Stop `xo-syncd` before copying or restoring its state directory. Server and native
state snapshots must include the durable Automerge replica. Markdown export is
the supported handoff from legacy Iroh workspaces; there is intentionally no
in-place transport-state migration.

Native mutable state is single-process locked. Do not run two `xo` processes
against the same state directory. Server and client directory backups are ordinary
stopped-process filesystem backups; Markdown import/export is provided by `xo`.

## Current limitations

- `xo-syncd` has no authentication or TLS termination; use an authenticating HTTPS
  reverse proxy for browser deployments.
