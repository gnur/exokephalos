# xo

**xo is a local-first knowledge workspace whose replicas synchronize directly
with one another over an end-to-end encrypted, peer-to-peer Iroh transport.**
It works without an application server, without a central database, and without
an online connection. A laptop, phone, browser tab, and always-on machine can
all be equal workspace peers.

The project has three clients:

- **`xo`** is a terminal workspace for creating, editing, searching, filtering,
  and resolving Markdown notes. It maintains a convenient Markdown projection,
  but the replicated Rust records and immutable revisions are authoritative.
- **`xo-web`** is an installable, fully client-side PWA. Its Rust/Wasm runtime
  and Iroh protocols run in a dedicated browser worker; the deployed site is
  only static HTML, JavaScript, Wasm, and assets. There is no application API or
  synchronization gateway behind it.
- **`xo-syncd`** is an optional always-on native peer. It keeps a durable replica
  available while laptops and phones sleep or disconnect. It is a convenience
  peer, not a server that owns the workspace or coordinates synchronization.

## What you can do with xo

xo is useful as a private notes and knowledge base, but its replicated document
model also supports workflows such as:

- write and read notes entirely offline, then synchronize later;
- keep a canonical Markdown directory on one or more native machines;
- use the TUI, a browser, and a phone against the same workspace;
- run a home server or workstation as an always-on `xo-syncd` peer;
- edit concurrently on disconnected devices and retain every branch until a
  user resolves the conflict;
- organize notes with tags, declarative views, subviews, predicates, sorting,
  and Rust-evaluated search;
- create plaintext or passphrase-encrypted notes, with ciphertext authenticated
  to its note identity and replicated without exposing the plaintext;
- import an existing Markdown tree, export a workspace, or use `xo-lsp` for
  diagnostics and note/tag completion in an editor;
- capture readable web pages into notes through capability-gated actions; and
- extend a workspace with sandboxed Steel modules and plugins, including the
  bundled Hardcover book-search workflow.

The same data can therefore support a solo offline workflow, a multi-device
mesh, a TUI-first setup with a headless replica, or a browser-first workflow.
Choose the peers that should be online; no peer is required to remain online for
local work.

## P2P and end-to-end encrypted transport

Workspace synchronization is **peer-to-peer**. Iroh attempts direct paths
between known endpoints and can use a relay only when the peers cannot connect
directly. A relay forwards encrypted traffic; it is not an xo application
server, does not host workspace APIs, and is not a database or lock manager.
Native and browser peers use the same Automerge and authenticated Iroh QUIC
protocols, so a browser synchronizes with native peers without a gateway.

The synchronized content travels over Iroh's end-to-end encrypted connections.
Relays provide connectivity but cannot read or resolve notes. Invitations carry
discovery information only: an invitation peer validates a new Ed25519
membership key and records its signed approval automatically. Every Automerge
change is signed, and
removed keys are permanently denied after their accepted causal frontier.
Read-only membership and bearer write capabilities are intentionally unsupported.

xo-syncd does not weaken this model. It is simply another authenticated replica
in the mesh. A browser can sync directly with it, two native clients can sync
with each other, and automatic peer discovery can form a full mesh as peers
learn about one another. When a device reconnects, immutable revisions and
Automerge records converge without requiring a central coordinator.

The Markdown directory is a **projection**, not the transport or complete
backup. Records, revision history, membership identities, and signed Automerge changes
live in the local state directory. Keep both the projection and state directory.

## Example workflows

### 1. Private offline notebook

Install `xo`, run `xo config-init`, and start the TUI. Create and edit notes in
`~/notes` while offline. Each save creates an immutable revision locally. When a
peer becomes reachable, the revisions synchronize automatically.

### 2. Browser and phone companion

Create a writable invitation in the TUI and scan its QR code with the PWA. The
capability is carried in the URL fragment, never sent in the HTTP request, and
is removed from the address bar after import. The browser stores its encrypted
identity in IndexedDB and can continue creating and editing notes offline. It
can later synchronize through an available native peer or another browser peer.

### 3. Always-on home or server peer

Run `xo-syncd` on a workstation, NAS, VPS, or small home server. Pair it once
with the TUI, then let laptops and phones come and go. The daemon holds a
persistent replica and resumes synchronization after restart; its loopback
operator page is only for administration and setup, not workspace transport.

### 4. Multi-device offline collaboration

Give each device its own state directory and peer identity. Two people—or two
of your own devices—can edit while disconnected. xo keeps concurrent revisions
and deterministically selects a visible winner without deleting the other
branch. Edit the note once with the desired content to create a descendant of
all branches and clear the conflict on every peer.

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
- `target/release/xo-syncd` — the persistent synchronization peer

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

Native multi-peer CI tests use an ephemeral in-process Iroh relay, while public N0 relay coverage remains an opt-in network test enabled with `XO_RUN_PUBLIC_IROH_TESTS=1`.

When systemd setup is selected, the installer prompts for both the workspace ID and workspace invitation before importing and starting the daemon. To seed it directly from a TUI invitation, provide both values:

```console
curl -fsSL https://xo.exokephalos.dev/install.sh \
  | XO_WORKSPACE_ID='<workspace-id>' XO_SYNC_TICKET='<writable-ticket>' bash
```

The installer imports that ticket into the user daemon state before enabling the service. The TUI pairing screen also displays this command after revealing the ticket; press `U` there to copy it. Tickets are secrets and should not be placed in shell history.

## Run the xo-web PWA

`xo-web` is a static client-side application with a typed dedicated-worker RPC
layer, sandboxed Steel, and direct browser Automerge/Iroh synchronization in Rust
WebAssembly. It can create an Automerge workspace, request authenticated
membership, synchronize through Iroh's end-to-end encrypted browser relay,
create and edit notes offline, recover its real Automerge replica and pending
revisions after reload, and converge browser and native peers. Endpoint and
membership keys plus the invitation are encrypted in IndexedDB. The PWA
uses no application service or application API.

The workspace restores the original mobile-first interaction model from the Go
web UI: sticky screen headers, dedicated item/tag/detail/editor panes,
URL-backed navigation, a bottom search/menu/create bar, rendered Markdown, and
compact settings. It loads replicated Steel view configuration, presents views
and subviews, and supports Rust-evaluated search and tag filtering. Notes can be
created, edited as frontmatter plus Markdown, deleted, restored, and inspected
through their revision and conflict history. Rust/Wasm validates records,
resolves heads, evaluates view predicates, and prepares immutable revision/head
writes; React owns only presentation. A raw document explorer remains available
for diagnostics. The header shows the embedded release tag. The PWA compares it with the uncached server version
on load, after a cached page is restored, when connectivity returns, and every
ten minutes. Only a changed deployment produces an explicit **Update** button.

### Pair a phone from the TUI

Run `setup_mobile_client` from the `:` action picker to create an invitation and display a QR code.
Scanning it opens `https://xo.exokephalos.dev/`, submits the browser's visible
peer ID and Ed25519 fingerprint for automatic admission, and stores its
encrypted identity and durable Automerge replica. The PWA polls while admission
is pending; synchronization starts immediately afterward and the invitation is
removed from the address bar. Invitation fragments are also handled when an already-open PWA receives
a new setup link; a sleeping peer no longer makes the invitation itself time out. The capability is encoded in the URL
fragment, so it is not included in the HTTP request. Treat the QR code and copied
setup link as secrets. On reload, cached records are immediately available and
entries authored by that browser are restored into its in-memory Iroh replica
before network synchronization. Remote signed entries remain cached without
being unsafely re-authored and refresh from peers in the background.

Settings includes **Wipe all browser data** for removing the encrypted identity,
workspace capability, document cache, pending writes, service worker, and offline
files. A new invitation is required afterward.

To use another PWA deployment, set its absolute HTTPS URL in
`~/.config/xo/config.scm`:

```scheme
(pwa-url "https://notes.example.test/")
```

The configured value must be an HTTPS origin without credentials, a path,
query parameters, or a fragment.

Build the Wasm package and static application locally with:

```console
cargo install wasm-pack --version 0.13.1 --locked
cd web
npm ci
npm run build:wasm
npm run build
```

### Deploy xo-web to Cloudflare Pages

Pushes to `main` and Git tags deploy the already-tested `xo-web` artifact
to the production branch of an existing Cloudflare Pages project. A tag embeds
its tag name as the PWA version, so publishing a release updates production to
that release after the browser/Wasm job passes. Pull requests never
deploy, and the job stays skipped until `CLOUDFLARE_PAGES_PROJECT` is set.
Configure these GitHub Actions repository settings before enabling the job:

- secret `CLOUDFLARE_API_TOKEN`: a custom Cloudflare token scoped to the target
  account with **Account → Cloudflare Pages → Edit**;
- secret `CLOUDFLARE_ACCOUNT_ID`: the target Cloudflare account ID; and
- variable `CLOUDFLARE_PAGES_PROJECT`: the existing Pages project name, not its
  domain or `pages.dev` URL.

The Pages project's production branch must be `main`. If Cloudflare Git
integration is also connected, disable its automatic production builds under
**Settings → Builds & deployments** to avoid two deployments per push. The job
downloads the artifact produced by the browser/Wasm test job rather than
rebuilding it, deploys with Wrangler, and verifies `/healthz` plus the uncached
`/version.json`. `web/public/_headers` applies the nginx-equivalent security and
cache headers when the static files are served by Pages. After configuring the
settings, run the **Build** workflow manually on `main` for the first deployment,
push another commit, or push a release tag.

Everything needed at runtime is written to `web/dist`. Any static host with SPA
fallback can serve it. The supplied nginx image is named `xo-web`:

```console
docker build -f Dockerfile.xo-web -t xo-web .
docker run --rm -p 8080:8080 xo-web
```

Open `http://127.0.0.1:8080`. The container has no writable workspace volume,
application process, API proxy, or server-side action executor. nginx serves
the versioned assets, Wasm, manifest, and application-shell service worker;
browser workspace durability belongs in IndexedDB.

## How synchronization works

### Automerge and the Markdown projection

An xo workspace is one Automerge document. Every TUI, PWA, and `xo-syncd`
instance keeps a durable local replica. A replica contains immutable note
revisions, per-author heads, workspace configuration, membership events,
device records, tombstones, and small asset bytes.

The Markdown directory is a local projection of that replicated state, not the
transport or source of truth. xo turns local Markdown edits into new immutable
revisions and materializes incoming revisions back into canonical Markdown
paths. This is why the projection remains editable while the machine is
offline.

A workspace invitation contains its protocol version, workspace ID, bootstrap
addresses, Gossip topic, and genesis-key fingerprint. A candidate submits its
peer ID, membership public key, and endpoint binding, then polls until automatic
admission is visible from an active invitation peer. Known peers and the durable Automerge replica survive
restart, so the invitation is not needed for every launch.

Synchronization is peer-to-peer and eventually consistent. Iroh attempts a
direct connection and can use its configured relay when a direct path is not
available. `xo-syncd` is therefore not a central database or lock server. It is
a stable, always-on replica that makes it easier for intermittently connected
TUI clients to exchange revisions. Two TUI clients can both edit offline and
converge after they reconnect.

### Conflict detection and resolution

Each note revision records its predecessor revisions. Normal sequential edits
form a chain; an ancestor head is history, not a conflict. If two peers edit the same note without seeing each
other's edit, both revisions remain heads and neither is an ancestor of the
other. xo records that as a conflict.

All peers choose the same visible revision without depending on message arrival
order. Candidates are ordered first by their hybrid logical clock (HLC) and
then by revision ID as a deterministic tie-breaker. The highest candidate is
the visible winner. This choice only keeps the UI and Markdown projection
stable—it does **not** discard or silently merge the other branch. Concurrent
revision IDs and all immutable history remain in the document. A concurrent
delete and edit are handled the same way, so both outcomes remain recoverable.

Run `open_conflicts` from the `:` action picker to see conflicted note IDs, the selected winner, concurrent
revision IDs, and revision history. To resolve a conflict, edit the visible note
and incorporate any content you want to retain. When xo saves a conflicted
note, the new revision names the winner and every concurrent revision as
predecessors. It is therefore a descendant of all branches; once that revision
replicates, every peer deterministically stops reporting the conflict. xo does
not attempt a line-by-line Markdown merge, so the user decides the final
content.

Revision history currently grows without a fixed limit and xo performs no
revision garbage collection. This is safe for offline peers and conflict
recovery, but a long-lived workspace with heavily edited notes will eventually
need explicit compaction. Compaction must retain current heads and unresolved
branches, establish a replicated checkpoint, and account for active or retired
offline peers before deleting predecessor records and unreferenced asset content; a
local "keep the last N revisions" deletion would break convergence and is not
implemented.

## Recommended setup: TUI first, then xo-syncd

The simplest setup starts with the first TUI as the workspace creator. Add the
always-on replica only after the local workspace is working, then optionally
join more TUI clients.

### 1. Create the first TUI workspace

Create the command configuration on the first client:

```console
mkdir -p ~/.config/xo
xo config-init > ~/.config/xo/config.scm
xo
```

The default configuration stores Iroh state in `~/.local/share/xo` and projects
Markdown into `~/notes`; TUI bindings are generated separately in `keys.scm`:

```scheme
(xo-config
  (schema 3)
  (state-dir "~/.local/share/xo")
  (workspace #f)
  (projection "~/notes")
  (pwa-url "https://xo.exokephalos.dev/"))
```

On this first launch, xo creates separate membership and Iroh identities plus a
local Automerge workspace, installs the default `xo.scm`, and opens
the TUI. Create or import some notes and verify that the local projection works
before adding a server. At this point the workspace is fully usable offline.

Keep the state directory as well as the Markdown projection. The state
directory contains endpoint and membership identities, the Automerge snapshot,
signed changes, and revision history; the projection alone is not a complete backup.

### 2. Start an empty xo-syncd service

Install and start `xo-syncd` on the always-on host. The systemd and container
options are documented under [Running xo-syncd](#running-xo-syncd). Do not seed
a second workspace: the pairing flow requests admission to the workspace created
by the first TUI.

The operator listener remains bound to loopback. Its workspace setup form does
not require an operator token; authenticated status and metrics APIs still use
the token created in the daemon state directory. This is an administrative HTTP
interface, not Iroh's synchronization port.

### 3. Pair the first TUI with xo-syncd

In the first TUI, run `open_server_setup` from the `:` action picker. xo immediately creates a writable
invitation and a ready-to-run user-unit installer command. Press `F2` to reveal
it and `U` to copy it, then run it on the daemon host. Alternatively, open
`http://127.0.0.1:9464/setup` and enter the displayed workspace ID and ticket.
For a remote host, first run
`ssh -L 9464:127.0.0.1:9464 user@server` and open the local URL.

The daemon imports the workspace, performs initial synchronization, and connects
back to the TUI. Neighbor discovery persists the peer relationship automatically;
there is no state-directory prompt or return ticket.

### 4. Optionally add more TUI clients

Each additional machine needs its own configuration and state directory:

```console
mkdir -p ~/.config/xo
xo config-init > ~/.config/xo/config.scm
```

Create an invitation from the server state while `xo-syncd` is stopped, then
restart it:

```console
sudo systemctl stop xo-syncd
sudo -u xo xo-admin invite /var/lib/xo-syncd '<WORKSPACE_ID>'
sudo systemctl start xo-syncd
```

Transfer the printed ticket privately and use it once on the new client:

```console
xo --ticket '<WRITABLE_TICKET>'
```

The first launch submits a membership request. Approve the displayed peer ID and
fingerprint from the `open_devices` action and retry the launch. It then
stores the Automerge workspace, starts synchronization, and opens the TUI.
Later launches use plain `xo`. Read-only membership is not supported. Never run
`xo-admin` and `xo-syncd` concurrently against the same state directory.

## Running xo-syncd

### Optional: seed a headless workspace first

The TUI-first flow above does not need this step. For a headless-first setup,
`xo-admin import-workspace` can create the replicated workspace used by the
server from a current projection or an empty directory. Markdown notes, assets,
and valid `xo.scm`, `modules/**/*.scm`, and `plugins/**/*.scm` configuration are
imported without modifying the source.

```console
mkdir -p /srv/xo-seed
mkdir -p /var/lib/xo-syncd
xo-admin import-workspace /srv/xo-seed /var/lib/xo-syncd
```

The command validates the complete source before creating replicated state. On
success it prints output similar to:

```text
workspace_id=<WORKSPACE_ID>
ticket=<WRITABLE_TICKET>
imported=0
assets=0
configs=0
```

Save both `workspace_id` and `ticket`. An invitation lets a candidate contact
the workspace, and an active invitation peer automatically grants membership to
the candidate's peer ID and Ed25519 fingerprint.

The state directory contains endpoint and membership identities, the Automerge
snapshot, and signed changes. Back it up and protect the identity files.

### Start the daemon directly

```console
xo-syncd \
  --state-dir /var/lib/xo-syncd \
  --operator-bind 127.0.0.1:9464
```

The daemon uses Iroh for synchronization. The operator address serves browser
setup, health, status, and Prometheus metrics; port `9464` is not the
synchronization port and does not need to be exposed to TUI clients.

Iroh discovers direct paths and can use its configured relay path when direct
connectivity is unavailable. The host needs outbound network access. A fixed
inbound sync port is not currently configured by xo.

On first start, `xo-syncd` creates
`/var/lib/xo-syncd/operator.token` with a random token. Its structured logs
report workspace setup attempts, successful initial synchronization, device
registration, resumed workspaces, incoming content, synchronization status
changes, and failures. A successful setup page is returned only after initial
synchronization completes; the daemon then connects back automatically. The public health checks are:

```console
curl http://127.0.0.1:9464/healthz
curl http://127.0.0.1:9464/readyz
```

Status and metrics require the bearer token:

```console
TOKEN="$(cat /var/lib/xo-syncd/operator.token)"
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:9464/v1/status
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:9464/v1/workspaces
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:9464/metrics
```

### Optional systemd services

The repository includes a hardened system service at
[`examples/systemd/xo-syncd.service`](examples/systemd/xo-syncd.service). It
uses the dedicated `xo` account and lets systemd create
`/var/lib/xo-syncd` with the correct ownership:

```console
sudo useradd --system --home-dir /var/lib/xo-syncd --shell /usr/sbin/nologin xo
sudo install -m 0755 target/release/xo-syncd /usr/local/bin/xo-syncd
sudo install -m 0644 examples/systemd/xo-syncd.service /etc/systemd/system/xo-syncd.service
sudo systemctl daemon-reload
sudo systemctl enable --now xo-syncd
sudo systemctl status xo-syncd
```

For a single-user machine, install
[`examples/systemd/xo-syncd-user.service`](examples/systemd/xo-syncd-user.service)
as a user unit instead:

```console
install -Dm0755 target/release/xo-syncd ~/.local/bin/xo-syncd
install -Dm0644 examples/systemd/xo-syncd-user.service \
  ~/.config/systemd/user/xo-syncd.service
systemctl --user daemon-reload
systemctl --user enable --now xo-syncd
```

The TUI pairing wizard described below supplies the values needed by the
daemon's browser setup page. The same flow works for system and user services.

### Container

The repository's Docker image contains only `xo-syncd`; the TUI and
administrative binaries are not installed. Build it locally with:

```console
docker build -t xo-syncd .
```

Run it with a named volume and publish the operator interface to host loopback:

```console
docker run --detach \
  --name xo-syncd \
  --restart unless-stopped \
  --publish 127.0.0.1:9464:9464 \
  --volume xo-syncd-data:/data \
  xo-syncd
```

The process runs as UID/GID `10001`, stores all durable state below `/data`,
and reports container health through `/readyz`. Open
`http://127.0.0.1:9464/setup` and follow the TUI pairing flow below. No operator
token is required for workspace setup; the generated `/data/operator.token`
continues to protect status and metrics APIs.
For a remote Docker host, use the same SSH port forwarding described in that
flow. Do not publish port `9464` on an unrestricted interface.

Pushes to `main` and tags publish multi-platform `linux/amd64` and
`linux/arm64` images to `ghcr.io/gnur/exokephalos`. Pull requests build the
same image without publishing it.

## Join an existing workspace with a ticket

This is the flow used by additional TUI clients and by a headless-first setup.
It is not needed on the first TUI in the recommended TUI-first flow.

### Create the local configuration

On the client machine:

```console
mkdir -p ~/.config/xo
xo config-init > ~/.config/xo/config.scm
```

The default configuration uses `~/.local/share/xo` for replicated local state
and `~/notes` for the Markdown projection. TUI bindings live in `keys.scm`:

```scheme
(xo-config
  (schema 3)
  (state-dir "~/.local/share/xo")
  (workspace #f)
  (projection "~/notes")
  (pwa-url "https://xo.exokephalos.dev/"))
```

### Join with a writable invitation

Use a workspace invitation generated by an existing peer:

```console
xo --ticket '<WRITABLE_TICKET>'
```

The first launch imports the workspace, starts synchronization with the peer
encoded in the ticket, records the workspace as the active local workspace, and
opens the TUI. The Markdown projection remains usable offline. Running `xo`
without the ticket later reopens that active workspace for local use:

```console
xo
```

After restarting `xo`, the TUI resumes live synchronization from Iroh's stored
peer list, so the ticket is only needed for the initial join. The ticket is
deliberately a command-line value rather than a persistent config field.

If the local state directory contains multiple workspaces, set the desired ID
in `~/.config/xo/config.scm`:

```scheme
(workspace "<WORKSPACE_ID>")
```

You can also select it for one launch:

```console
xo --workspace '<WORKSPACE_ID>'
```

The uncluttered TUI header shows only xo and the embedded release version.
The TUI subscribes to Iroh document events and automatically reloads notes,
conflicts, devices, replicated behavior, and the filesystem projection when
local or remote content becomes available. TUI, browser, and `xo-syncd` peers
publish signed device records when they open a workspace, so `open_devices`
shows the clients that have joined after their records replicate. Run
`open_sync_status` for detailed synchronization state or `refresh_sync` for a
manual refresh and retry.

Create and edit commands open a private temporary file whose name ends in
`.xo.md`. Editors can associate that compound extension with `xo-lsp` while the
ordinary projected notes retain their canonical `.md` names.

Press `c` to create a plaintext note or `C` to create an encrypted note. New
encrypted notes require a confirmed non-empty passphrase before the editor
opens. The editor receives the complete frontmatter and plaintext body; xo
restores the authoritative ID and creation timestamp, encrypts only the edited
body, and commits the first revision only after encryption. Editing an existing
encrypted note follows the same full-document workflow and re-encrypts it with a
fresh salt and nonce. xo deliberately provides no conversion from an existing
plaintext note to an encrypted note because plaintext revisions would remain in
history. Editors may create their own swap, backup, or recovery files, so those
features should be configured appropriately for `.xo.md` files.

### TUI actions, key bindings, navigation, and tag filtering

Every normal-mode interaction is a named action. Press `:` to open the
autocompleting action picker, type part of an action name, use Up/Down to select,
Tab to complete, and Enter to run it. Actions include `cursor_down`,
`focus_column_left`, `focus_subview_next`, `edit_item`, `open_search`,
`delete_item`, `open_view_picker`, `open_goto`, `open_item_actions`,
`edit_workspace_config`, `setup_mobile_client`, `open_server_setup`,
`open_sync_status`, `open_conflicts`, `open_devices`, `refresh_sync`,
`reverse_sort`, and `unlock_preview`.

The TUI creates `~/.config/xo/keys.scm` on first start and hot reloads it while
running. Bind keys to actions with declarative forms:

```scheme
(keys
  (bind "j" cursor_down)
  (bind "h" focus_column_left)
  (bind "tab" focus_subview_next)
  (bind "e" edit_item)
  (bind "/" open_search)
  (bind "d" delete_item)
  (bind "g" open_view_picker)
  (bind ":" action_picker)
  (bind "b" (goto_view "books/read")))
```

Names such as `space`, `enter`, `tab`, `backtab`, `left`, `right`, `up`, and
`down` represent special keys; modifier forms such as `ctrl+x` are also
accepted. The footer is generated from the active bindings and changes after a
successful hot reload. Invalid edits leave the previous keymap active and show
a reload error. The legacy `(leader-key ...)` command setting is accepted for
configuration compatibility but no longer opens a leader menu.

By default `g` opens the top-level view switcher, Tab and Shift-Tab cycle
subviews, and Left/Right or `h`/`l` move between Tags, Notes, and Preview.
Highlight a tag with Up/Down or `j`/`k`; Space toggles that tag and does nothing
outside the tag column.

Tag counts are live facets. They first respect the active view or subview and
the `/` title query, then show how many notes would remain if each tag were
added to the currently selected tag filters. Selecting or clearing a tag
therefore updates every displayed count immediately.

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
revision. Other peers receive the configuration through Iroh; use the TUI refresh
command after a remote configuration update.

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

## Detailed TUI-to-xo-syncd pairing flow

If the workspace was created in the TUI, run `open_server_setup` to open **Connect
xo-syncd**. The workspace invitation is hidden by default.

1. Press `F2` to reveal the ticket and generated installer command.
2. Press `U` to copy the installer command and run it on the Linux daemon host.
   The script asks for any workspace ID or ticket not already supplied by the
   command, verifies that they match, imports the workspace into the daemon's
   separate state directory, and starts the systemd user unit.
3. Alternatively, open `http://127.0.0.1:9464/setup`, entering the displayed
   workspace ID and workspace invitation. For a remote daemon, temporarily forward
   its loopback listener:

   ```console
   ssh -L 9464:127.0.0.1:9464 user@server
   ```

The setup form requires no operator token because the listener is loopback-only.
It validates that the workspace invitation belongs to the entered workspace, imports
it, and waits for initial synchronization. The daemon then connects back to the
TUI automatically, so there is no return ticket to paste. Press `c` to copy only
the ticket or Enter/Esc to close the pairing screen. The setup page does not
store the ticket in browser storage.

The TUI and mutating `xo` commands such as `xo import` use an exclusive lock
inside the state directory. If another `xo` process is already using that
workspace, the second process exits with a clear error. Never run `xo-admin`
and `xo-syncd` concurrently against the same state directory.

## Operations and recovery

Administrative commands should be run while the daemon is stopped:

```console
# Inspect workspace records and projection diagnostics.
xo-admin diagnostics /var/lib/xo-syncd '<WORKSPACE_ID>'

# Create and verify an offline backup.
xo-admin backup /var/lib/xo-syncd /srv/backups/xo-2026-07-22
xo-admin verify-backup /srv/backups/xo-2026-07-22

# List devices known to the workspace.
xo-admin device-list /var/lib/xo-syncd '<WORKSPACE_ID>'
```

Backups are offline snapshots: stop `xo-syncd` before creating or restoring
one. Restore into a new or empty state directory with:

```console
xo-admin restore /srv/backups/xo-2026-07-22 /var/lib/xo-syncd-restored
```

## Current limitations

- The operator server is plain HTTP and binds to loopback by default. Keep it on
  loopback and use an SSH tunnel, or place it behind a suitably secured reverse
  proxy.
- Ticket revocation is not equivalent to deleting a string that has already
  been shared. Use permanent membership-key removal when a capability or
  device must be revoked.
- The binaries are not yet packaged by this repository; build or deploy the
  release binaries directly.
