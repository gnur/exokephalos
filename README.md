# xo

**xo is an offline-first knowledge workspace backed by Automerge.** Each client
keeps a durable local replica and can create or edit notes without connectivity.
All replicas synchronize through one `xo-syncd` server per workspace over the
shared `/api/sync` WebSocket endpoint.

The project has three clients:

- **`xo`** is the terminal workspace and Markdown projection. Connect it with
  `xo --server https://notes.example.test` (the default is
  `http://127.0.0.1:9464`).
- **`xo-web`** is an installable offline-first PWA. During the transport migration
  its worker is being moved to the same-origin `/api/sync` endpoint.
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

The Iroh removal is intentionally breaking and still in progress. See
[`iroh-removal-plan.md`](iroh-removal-plan.md) for completed and remaining work.

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
cargo build --release -p xo -p xo-admin -p xo-syncd
```

The resulting programs are:

- `target/release/xo` — the terminal UI
- `target/release/xo-admin` — offline workspace administration
- `target/release/xo-syncd` — the centralized synchronization and web server

The examples below assume those binaries have been copied somewhere in
`PATH`.

GitHub Actions builds release archives for Linux x86-64, Linux ARM64, macOS
ARM64, and Windows x86-64. Pushing a UTC timestamp tag creates the corresponding
GitHub Release automatically with generated release notes, all four archives,
the static PWA, and a `SHA256SUMS` file. The exact tag is embedded as the version
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

You can install the native `xo` TUI, `xo-syncd` background daemon, `xo-admin`, and `xo-lsp` directly from the deployed static site:

```console
curl -sSL https://xo.exokephalos.dev/install.sh | bash
```

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

Its transport is currently the largest unfinished migration area: the checked-in
browser runtime still contains transitional Iroh invitation, relay, membership,
and signed-change code. It is being replaced by an IndexedDB-backed Automerge
replica that connects automatically to same-origin `/api/sync`. Do not treat the
legacy invitation workflow as part of the centralized architecture.

Production PWA assets are embedded in `xo-syncd`; there is no separate static
production deployment. Open the authenticated origin serving the daemon. Static
assets and cached UI remain client-side, while synchronization and the item API
share that origin.

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

Start one daemon for the workspace:

```console
xo-syncd --state-dir ~/.local/share/xo-syncd --bind 127.0.0.1:9464
```

Create native configuration and connect the TUI:

```console
mkdir -p ~/.config/xo
xo config-init > ~/.config/xo/config.scm
xo --server http://127.0.0.1:9464
```

Each native client needs its own state and projection directories but points at
the same server. `--server` is the only native server-discovery mechanism. There
are no tickets, invitations, approvals, endpoint identities, or pairing steps.

`open_peers` shows currently connected client IDs. These are presence labels;
they do not grant or revoke access. Protect browser access with an authenticating
HTTPS reverse proxy and expose WebSocket upgrades for `/api/sync`. `xo-syncd`
itself intentionally trusts every request that reaches it.

### Server routes

- `GET /healthz` returns exactly `ok\n`.
- `GET /api/sync` upgrades to centralized Automerge synchronization.
- `GET /api/items/{id}` returns an item.
- `POST /api/items` safely captures a public URL.
- `PATCH /api/items/{id}` updates supplied fields through a new revision.
- `DELETE /api/items/{id}` creates a deleted revision.
- Other GET routes serve the embedded PWA with SPA fallback.

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
for example, `:q` runs `quit`, while `:p` opens peer management.

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
projection, Automerge snapshot, signed-change log, and local index have been finalized and closed.

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
Executable sandboxed plugins live below `plugins/**/*.scm`; their
`xo-plugin-manifest` function contributes actions and their action entrypoint
runs in a fresh, time-bounded `Engine::new_sandboxed()` VM.

The `capture-url` plugin is a capability-gated native host action. Run `open_item_actions` from the `:` action picker,
select **Capture readable content from a URL**, and enter an HTTP or HTTPS URL.
The host validates public destinations and redirects, limits the response,
extracts the readable article, converts it to Markdown, and commits an ordinary
replicated note. Both `create-note` and `network` grants are required. Steel
itself receives no ambient network API.

### Hardcover Steel plugin

Install the bundled plugin into the active replicated workspace and provide its
API token to the `xo` process:

```console
xo plugin install hardcover
export HARDCOVER_TOKEN='your Hardcover API token'
xo
```

Run `open_item_actions` from the `:` action picker and select **Search Hardcover**. The plugin prompts for a title or
author, performs the GraphQL request, presents up to five choices, and creates
an ordinary `type: book` note tagged `to-read`. The plugin exposes only this
search action; reading-state actions belong to workspace configuration.

`plugins/hardcover.scm` contains the GraphQL request, JSON traversal, metadata
normalization, result labels, and note fields. Rust provides only generic,
capability-checked `xo-secret` and `xo-http-post-json` host functions. Plugin
HTTP is HTTPS-only, proxy-free, DNS-pinned to validated public addresses,
redirect-free, time-bounded, header-restricted, and limited to 2 MiB. The
plugin requires `create-note`, `network`, and `read-secret`; Steel receives no
filesystem, process, socket, or dylib access.

`example-config.scm` is a complete multi-view example with nested predicates,
subviews, sorting, explicit mutation grants, and reading actions that set
`started` and `finished` to their host-supplied execution timestamp.

The declarative workspace/module form remains restricted. Configuration is parsed through a
strict boundary: arbitrary filesystem, environment, process, network, clock,
or evaluation expressions are rejected. Auxiliary modules and plugins remain
materialized below `modules/**/*.scm` and `plugins/**/*.scm`; the main workspace
configuration is kept in replicated state and edited with the `edit_workspace_config` action.

## Operations and recovery

Stop `xo-syncd` before copying or restoring its state directory. Server and native
state snapshots must include the durable Automerge replica. Markdown export is
the supported handoff from legacy Iroh workspaces; there is intentionally no
in-place transport-state migration.

Native mutable state is single-process locked. Do not run two `xo` processes
against the same state directory. `xo-admin` still contains legacy Iroh-oriented
commands and is scheduled for removal or centralized redesign.

## Current limitations

- The browser worker still uses transitional Iroh code and does not yet implement
  the target IndexedDB Automerge `/api/sync` client.
- `xo-syncd` has no authentication or TLS termination; use an authenticating HTTPS
  reverse proxy for browser deployments.
- Remaining Iroh, membership, invitation, and signed-change modules in `xo-core`,
  `xo-web`, and `xo-admin` are migration leftovers.
- Browser/server convergence, browser offline reconnect, and browser conflict tests
  remain incomplete.
