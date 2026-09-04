# xo

`xo` is an offline-first knowledge workspace backed by Automerge. Native and
browser clients keep durable local replicas, accept edits without connectivity,
and synchronize through one self-hosted `xo-syncd` workspace. Concurrent
revisions and conflicts are retained instead of being overwritten.

- **`xo`** — terminal UI, local Markdown projection, import/export, and plugin
  manager.
- **`xo-syncd`** — authoritative synchronization server, OAuth-protected item
  API, webhook receiver, and host for the embedded PWA.
- **`xo-pwa`** — installable browser client served by `xo-syncd`.
- **`xo-lsp`** — diagnostics and completion for projected Markdown.

## Quick start

### 1. Install xo

Download and run the release installer:

```console
curl -fsSL https://raw.githubusercontent.com/gnur/exokephalos/main/install.sh -o install.sh
bash install.sh
```

It installs binaries in `~/.local/bin` and can create a systemd user service for
`xo-syncd`. On later runs it stops a running daemon before replacing its binary.
If server configuration already exists, choose an in-place upgrade to preserve
it or a fresh setup that creates timestamped configuration and state backups.

### 2. Configure Pocket ID

Create an API resource in Pocket ID for your public xo origin, for example
`https://notes.example.com`, with these permissions:

- `xo:read`
- `xo:write`
- `xo:sync`

Create a **public** OIDC client that allows authorization code with PKCE and
refresh tokens. Grant it all three API permissions and register these exact
callback URLs:

```text
https://notes.example.com/
http://127.0.0.1:9465/callback
```

The HTTPS URL is used by the PWA. The fixed loopback URL is used when `xo` opens
the browser for native login.

### 3. Set up the synchronization server

Select the `xo-syncd` option in the installer and enter the Pocket ID values, or
create `~/.config/xo-syncd/config.scm` yourself:

```scheme
(xo-syncd-config
  (schema 1)
  (state-dir "~/.local/share/xo-syncd")
  (bind "127.0.0.1:9464")
  (oidc-issuer "https://id.example.com")
  (oidc-audience "https://notes.example.com")
  (oidc-client-id "YOUR_PUBLIC_CLIENT_ID"))
```

Generate the same template with:

```console
mkdir -p ~/.config/xo-syncd
xo-syncd config-init > ~/.config/xo-syncd/config.scm
systemctl --user enable --now xo-syncd
curl -fsS http://127.0.0.1:9464/healthz
```

Put an HTTPS reverse proxy in front of `127.0.0.1:9464`. It must preserve
`Authorization`, `Sec-WebSocket-Protocol`, and normal WebSocket upgrade headers.
`xo-syncd` validates OAuth tokens but deliberately leaves TLS termination to the
proxy.

### 4. Connect the native client

Create `~/.config/xo/config.scm`:

```scheme
(xo-config
  (schema 5)
  (state-dir "~/.local/share/xo")
  (client-id #f)
  (server "https://notes.example.com")
  (projection "~/notes"))
```

Then run:

```console
xo
```

The first login opens Pocket ID in your browser. Native OAuth credentials are
stored in `~/.config/xo/auth.json` with mode `0600`. Open
`https://notes.example.com/` for the PWA and install it from the browser if
wanted.

### 5. Configure two item types

Workspace behavior is replicated to every client. In the TUI, open the `:`
action picker and run `edit_workspace_config`. This minimal configuration
manages notes and books as separate views:

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
      (subviews))
    (view
      (id "books")
      (name "Books")
      (key "b")
      (show-tags #t)
      (title-field "title")
      (subtitle-field "author")
      (sort-field "created")
      (descending #t)
      (preview #f)
      (predicate (field-equals "type" "book"))
      (subviews)))
  (actions)
  (templates)
  (capability-grants))
```

See [`example-config.scm`](example-config.scm) for subviews, compound predicates,
sorting, and actions that move books through a reading workflow.

## Using xo

### Offline synchronization and storage

A write is acknowledged only after the local replica is durable. Clients
reconnect to `/api/sync` with bounded backoff and exchange Automerge sync
messages. `xo-syncd` persists accepted changes before notifying other clients.
HLC ordering selects a visible revision while concurrent heads remain available
in conflict history.

The Markdown directory is a projection, not the synchronization transport or a
complete backup. Back up both server state (`~/.local/share/xo-syncd`) and each
native client's state (`~/.local/share/xo`) with the corresponding process
stopped. Do not run two native processes against one client state directory.

### TUI navigation and actions

Press `:` to open the action picker, type to filter, use Up/Down to select, and
press Enter to run the highlighted action. Useful defaults include:

- `g` — choose a view.
- Tab / Shift-Tab — cycle subviews.
- `h` / `l` or Left / Right — move between Tags, Notes, and Preview.
- `/` — search titles.
- `c`, `e`, `d`, `u` — create, edit, delete, and restore.
- `p` — show connected clients.
- `q` — quit.

The keymap is created at `~/.config/xo/keys.scm` and hot-reloaded. Generate it
explicitly with `xo keymap-init`. Bind keys to named actions with forms such as:

```scheme
(keys
  (bind "j" cursor_down)
  (bind "k" cursor_up)
  (bind "tab" focus_subview_next)
  (bind "backtab" focus_subview_previous)
  (bind "b" (goto_view "books/all"))
  (bind ":" action_picker)
  (bind "q" quit))
```

### Workspace behavior

Views and subviews filter items with `always`, `field-equals`, `has-tag`, `not`,
`all`, and `any` predicates. They can choose title, subtitle, and sort fields.
Actions can `add-tag`, `remove-tag`, `set-field`, and `append-body`.
`(set-field "started" (now))` uses a host-supplied RFC 3339 timestamp.

Any action that changes an item needs an explicit capability grant:

```scheme
(actions
  (action
    (id "mark-done")
    (description "Mark item as done")
    (predicate (has-tag "todo"))
    (effects (add-tag "done"))))
(capability-grants
  (grant
    (action "mark-done")
    (capabilities mutate-note)))
```

Workspace configuration and optional `modules/**/*.scm` are replicated. Local
executable plugins are not replicated.

### Import and export

Import a copy of an existing Markdown tree, never the active projection itself:

```console
xo import ~/incoming-notes
xo import ~/incoming-books --type book
```

`xo` validates all files before committing any item, leaves the source
untouched, assigns missing IDs and required frontmatter, and reports collisions.

Export current winning revisions to a new or empty directory:

```console
xo export ~/xo-export
xo export ~/books-export --type book
```

Output is grouped by item type, year, and month. Export is the portable handoff
format for legacy workspaces.

### Editor integration

Launch `xo-lsp` as a stdio language server for the projection. It reports
frontmatter and ID problems and completes `[[note-links]]` and tags. Use
`xo-lsp --workspace ~/notes` when an editor does not provide a workspace folder.

## Installing and managing plugins

Executable Steel plugins are local to each native client and stored as direct
`.scm` files in:

```text
~/.config/xo/plugins/
```

Manage them without manually copying files:

```console
xo plugin list
xo plugin install example ./example.scm
xo plugin update example ./new-example.scm
xo plugin remove example
```

Use `-` to install from standard input. For example, install the included
Hardcover integration directly from this repository:

```console
curl -fsSL https://raw.githubusercontent.com/gnur/exokephalos/main/plugins/hardcover.scm \
  | xo plugin install hardcover -
export HARDCOVER_TOKEN='your-token'
xo
```

The plugin contributes `hardcover-search` to the main action list. Run it from
`:` and enter a title or author. Put `HARDCOVER_TOKEN` in the environment used
to launch `xo`; the token is read only because the plugin declares the
`read-secret` capability.

Install validates the manifest and refuses to replace an existing plugin;
`update` validates before atomically replacing it. All valid `.scm` files in the
directory are auto-discovered at startup and register their declared actions.
Plugins are not copied to `xo-syncd` or synchronized, so install them separately
on every native client that should provide those actions.

The bundled [`plugins/manage-tags.scm`](plugins/manage-tags.scm) is another
example. Install it, select notes with the `toggle_selection` action, then run
`manage-tags` to apply tags through xo's native multi-item picker.

## Plugin maker guide

### Steel environment

Each manifest and action executes in a size- and time-bounded Steel
`Engine::new_sandboxed()` VM. Plugin code can use Steel's ordinary language
features—functions, lexical bindings, control flow, lists, vectors, hash maps,
strings, numbers, and JSON conversion with `string->jsexpr` and
`value->jsexpr-string`.

There is no ambient filesystem, environment, process, socket, terminal, or clock
access. Network, secrets, item reads, and mutations are available only through
xo host functions and declared capabilities. xo owns prompts, result selection,
persistence, IDs, immutable revisions, and TUI rendering.

### Plugin contract

A plugin defines `xo-plugin-manifest`, which returns a JSON string:

```scheme
(define (xo-plugin-manifest)
  "{\"schema\":1,\"actions\":[{\"id\":\"example-search\",\"description\":\"Search an example service\",\"prompt\":\"Search terms\",\"entrypoint\":\"xo-plugin-run\",\"capabilities\":[\"network\",\"create-note\"]}]}")
```

Each action may declare:

- `id` — unique action name.
- `description` — text shown in action lists.
- `prompt` — native input prompt.
- `entrypoint` — Steel function name; defaults to `xo-plugin-run`.
- `predicate` — optional item predicate.
- `effects` — optional declarative workspace effects.
- `capabilities` — requested host capabilities.
- `interaction` — `prompt` (default) or `tag-picker`.

A prompt entrypoint receives the user's input string and returns a JSON string
with `choices` and optional `operations`:

```scheme
(define (xo-plugin-run input)
  (value->jsexpr-string
    (hash "choices"
      (list
        (hash "label" input
              "note" (hash "frontmatter" (hash "type" "note"
                                                "title" input)
                           "body" "Created by a plugin"))))))
```

Each choice has a display `label` and a note containing `frontmatter` and `body`.
The selected choice is committed as a normal replicated item.

### xo host functions

Host functions exchange JSON strings so the boundary remains explicit:

```text
(xo-selected-item-ids)             JSON array of selected item IDs
(xo-note-content id)               JSON object with frontmatter and body
(xo-all-tags)                      JSON array of known tags
(xo-secret name)                   value of an A-Z/0-9/_ environment variable
(xo-http-get url headers)          bounded HTTPS GET
(xo-http-post-json url headers body)
(xo-update-items operations)       queue update-item operations
(xo-create-item operation)         queue one create-item operation
```

Each host call checks the capabilities declared for the running action; access
that was not declared is denied:

| Capability | Host access |
| --- | --- |
| `read-secret` | `xo-secret` |
| `network` | `xo-http-get`, `xo-http-post-json` |
| `mutate-note` | `xo-update-items` and declarative mutation effects |
| `create-note` | `xo-create-item` and returned note choices |

HTTP access is HTTPS-only, proxy-free, DNS-pinned to validated public addresses,
redirect-free, time-bounded, header-restricted, and limited to 2 MiB. Plugins
cannot bypass these controls with Steel APIs.

Install a plugin under a temporary name while developing, restart `xo`, and run
its action from `:`:

```console
xo plugin install my-plugin ./my-plugin.scm
xo plugin update my-plugin ./my-plugin.scm
```

Manifest errors are reported by the management command before installation.
Runtime errors are shown in the TUI without granting the plugin additional host
access.

## Server API and operation

Public routes:

- `GET /healthz` — returns `ok`.
- `GET /.well-known/xo-configuration` — non-secret OIDC client settings.
- `POST /api/webhook/{source}` — creates a webhook item; protect it with proxy
  rate and body limits when exposed publicly.
- Other `GET` routes — embedded PWA with SPA fallback.

Authenticated routes:

- `GET /api/sync` — WebSocket sync; requires `xo:read`, `xo:write`, and `xo:sync`.
- `GET /api/items/{id}` — read an item.
- `POST /api/items` — safely capture a public URL.
- `PATCH /api/items/{id}` — create an updated revision.
- `DELETE /api/items/{id}` — create a deleted revision.

Send access tokens as `Authorization: Bearer …`. Reads require `xo:read` and
writes require `xo:write`. JSON bodies are limited to 1 MiB. URL capture checks
DNS and every redirect and rejects private or special addresses.

To run the combined daemon, API, and PWA container:

```console
docker run --rm -p 9464:9464 -v xo-data:/data \
  ghcr.io/gnur/xo-syncd:latest \
  --state-dir /data --bind 0.0.0.0:9464 \
  --oidc-issuer https://id.example.com \
  --oidc-audience https://notes.example.com \
  --oidc-client-id YOUR_PUBLIC_CLIENT_ID
```

## Building from source

Rust 1.89 or newer is required:

```console
cargo build --release -p xo -p xo-lsp -p xo-syncd
```

To build the embedded PWA locally:

```console
cargo install wasm-pack --version 0.13.1 --locked
cd web
npm ci
npm run build:wasm
npm run build
cd ..
XO_PWA_DIR="$PWD/web/dist" cargo build --release -p xo-syncd
```

Without `XO_PWA_DIR`, development daemon builds contain a diagnostic fallback
page. Release binaries and published containers embed the tested production
PWA.

## Current limitations

- One `xo-syncd` process hosts one workspace.
- TLS termination requires an HTTPS reverse proxy.
- The public webhook route does not authenticate senders.
- Local mutable native state is single-process locked.
