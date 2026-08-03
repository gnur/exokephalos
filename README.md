# xo

xo is an offline-first personal knowledge workspace. The `xo` terminal UI keeps
an ordinary Markdown projection on disk, while Iroh provides replicated state
and peer-to-peer synchronization. `xo-syncd` is an always-on peer: it stores a
copy of a workspace and gives intermittently connected TUI clients a stable peer
with which to synchronize. `xo-web` is the fully client-side, installable PWA;
its Rust/Wasm runtime runs in a dedicated browser worker and its deployable
output consists only of static files.

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

## Run the xo-web PWA

`xo-web` is a static client-side application with a typed dedicated-worker RPC
layer, sandboxed Steel, and direct browser Iroh Docs/Blobs/Gossip in Rust
WebAssembly. It can create a writable document, join an existing writable
ticket, synchronize through Iroh's end-to-end encrypted browser relay, create
and edit notes offline, recover cached records and pending revisions after reload,
and converge two browser contexts through a native `xo-syncd` peer. Endpoint and
author keys plus the writable capability are encrypted in IndexedDB. The PWA
uses no application service or application API.

The workspace UI loads replicated Steel view configuration, presents views and
subviews, and supports Rust-evaluated search and tag filtering. Notes can be
created, edited as frontmatter plus Markdown, deleted, restored, and inspected
through their revision and conflict history. Rust/Wasm validates records,
resolves heads, evaluates view predicates, and prepares immutable revision/head
writes; React owns only presentation. A raw document explorer remains available
for diagnostics. The footer shows the embedded release tag. The PWA compares it with the uncached server version
on load, after a cached page is restored, when connectivity returns, and every
ten minutes. Only a changed deployment produces an explicit **Update** button.

### Pair a phone from the TUI

Press `Space`, then `m` in the TUI to create a writable invitation and display a QR code.
Scanning it opens `https://xo.exokephalos.dev/`, imports the writable capability,
starts relay synchronization, stores the encrypted browser identity, and removes
the capability from the address bar. The capability is encoded in the URL
fragment, so it is not included in the HTTP request. Treat the QR code and copied
setup link as secrets.

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

### Iroh documents and the Markdown projection

An xo workspace is an [Iroh Docs](https://www.iroh.computer/docs) namespace.
Every TUI and `xo-syncd` instance keeps a persistent local replica in its own
state directory. A replica contains:

- immutable note revisions and one current head per author;
- workspace configuration, device records, and asset metadata; and
- content hashes whose bytes are transferred through Iroh Blobs.

The Markdown directory is a local projection of that replicated state, not the
transport or source of truth. xo turns local Markdown edits into new immutable
revisions and materializes incoming revisions back into canonical Markdown
paths. This is why the projection remains editable while the machine is
offline.

An Iroh ticket contains the document capability, workspace ID, and addressing
information for one or more peers. A writable ticket grants write access to the
whole workspace; treat it like a secret. A read-only ticket can replicate data
but cannot publish revisions. Tickets are needed to establish a peer
relationship, not for every launch: Iroh stores known peers in the state
directory and xo resumes synchronization on restart.

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

Press `Space`, then `x` in the TUI to see conflicted note IDs, the selected winner, concurrent
revision IDs, and revision history. To resolve a conflict, edit the visible note
and incorporate any content you want to retain. When xo saves a conflicted
note, the new revision names the winner and every concurrent revision as
predecessors. It is therefore a descendant of all branches; once that revision
replicates, every peer deterministically stops reporting the conflict. xo does
not attempt a line-by-line Markdown merge, so the user decides the final
content.

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

The default configuration stores Iroh state in `~/.local/share/xo`, projects
Markdown into `~/notes`, and uses Space as the TUI leader key:

```scheme
(xo-config
  (schema 3)
  (state-dir "~/.local/share/xo")
  (workspace #f)
  (projection "~/notes")
  (pwa-url "https://xo.exokephalos.dev/")
  (leader-key " "))
```

On this first launch, xo creates a local Iroh endpoint and writable document,
records it as the active workspace, installs the default `xo.scm`, and opens
the TUI. Create or import some notes and verify that the local projection works
before adding a server. At this point the workspace is fully usable offline.

Keep the state directory as well as the Markdown projection. The state
directory contains the endpoint identity, document capabilities, revision
history, and blobs; the projection alone is not a complete backup.

### 2. Start an empty xo-syncd service

Install and start `xo-syncd` on the always-on host. The systemd and container
options are documented under [Running xo-syncd](#running-xo-syncd). Do not seed
a second workspace: the pairing flow imports the document created by the first
TUI.

The daemon creates an operator token on first start. For the system service it
is `/var/lib/xo-syncd/operator.token`; for the documented container it is
`/data/operator.token`. Keep the operator listener on loopback. It is an
administrative HTTP interface, not Iroh's synchronization port.

### 3. Pair the first TUI with xo-syncd

In the first TUI, press `Space`, then `j` and follow the three-step **Connect xo-syncd**
wizard. In outline:

1. Enter the server state directory: normally `/var/lib/xo-syncd` for the
   system service or `/data` for the documented container.
2. xo creates a one-time writable invitation. Open
   `http://127.0.0.1:9464/setup`, enter the operator token, workspace ID, and
   invitation, and submit the form. For a remote host, first run
   `ssh -L 9464:127.0.0.1:9464 user@server` and open the local URL.
3. The server imports the workspace and returns its own writable ticket. Paste
   that ticket into the TUI to complete the connection in the other direction.

The two-ticket exchange gives each endpoint current addressing information and
persists the peer relationship on both sides. After it succeeds, future TUI and
daemon launches resume synchronization without either ticket. See the
[full pairing walkthrough](#detailed-tui-to-xo-syncd-pairing-flow) for ticket
visibility controls and the command-line fallback.

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

That launch imports the existing Iroh document, records it as the active local
workspace, starts synchronization, and opens the TUI. Later launches use plain
`xo`. Repeat these steps for any other optional clients. Add `--read-only` to
`xo-admin invite` when a client should replicate but not publish changes. Never
run `xo-admin` and `xo-syncd` concurrently against the same state directory.

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

Save both `workspace_id` and `ticket`. A writable ticket is a capability: anyone
who possesses it can join and write to the workspace. Transfer it privately and
do not commit it to a repository or put it in `config.scm`.

The state directory contains the server's endpoint identity, workspace records,
and blobs. Back it up and do not delete `endpoint.key`.

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
`/var/lib/xo-syncd/operator.token` with a random token. The public health checks
are:

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
and reports container health through `/readyz`. Read the generated operator
token with:

```console
docker exec xo-syncd cat /data/operator.token
```

Then open `http://127.0.0.1:9464/setup` and follow the TUI pairing flow below.
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

The default configuration uses `~/.local/share/xo` for replicated local state,
`~/notes` for the Markdown projection, and Space as the TUI leader key:

```scheme
(xo-config
  (schema 3)
  (state-dir "~/.local/share/xo")
  (workspace #f)
  (projection "~/notes")
  (pwa-url "https://xo.exokephalos.dev/")
  (leader-key " "))
```

### Join with the server ticket

Use the writable ticket printed while initializing the server:

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
Press `Space`, then `s` for detailed synchronization state or `Space`, then `r`
to refresh and retry synchronization.

### TUI leader, navigation, and tag filtering

Pressing the configured leader opens a popup listing views, tags, actions,
mobile setup, server setup/status, synchronization status, conflicts, devices,
refresh, sorting, and preview unlocking. Space is the default. Set another
single printable character with `(leader-key ",")` in the schema-3 command
configuration.

Press `Space`, then `v` to open the view menu. Every configured view and subview
is shown with its shortest unique prefix; type that prefix to switch immediately,
or use the arrow keys and Enter.

Press `Space`, then `t` to show or hide the tag pane. When it is visible, `Tab`
and `Shift-Tab` include it in cyclic pane navigation. Use Left/Right or `h`/`l`
for spatial pane movement between Tags, Notes, and Preview. Highlight a tag with
Up/Down or `j`/`k`, then press Enter to toggle that filter.

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

Workspace behavior is replicated and projected as `xo.scm`. A new workspace
starts with native declarative Steel similar to:

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

The `capture-url` plugin is a capability-gated native host action. Press `Space`, then `a`,
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

Press `Space`, then `a` and select **Search Hardcover**. The plugin prompts for a title or
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
or evaluation expressions are rejected.

## Detailed TUI-to-xo-syncd pairing flow

If the workspace was created in the TUI, press `Space`, then `j` to open **Connect
xo-syncd**:

1. Confirm the server state directory. The default is `/var/lib/xo-syncd`.
2. Press Enter to create a writable invitation. The ticket is hidden by
   default.
3. Open `http://127.0.0.1:9464/setup` on the server. For a remote server, keep
   the operator endpoint on loopback and forward it temporarily:

   ```console
   ssh -L 9464:127.0.0.1:9464 user@server
   ```

4. Enter the operator token, workspace ID, and writable ticket displayed by
   the TUI. For the system unit, read the token on the server with:

   ```console
   sudo cat /var/lib/xo-syncd/operator.token
   ```

   Press `c` in the TUI to copy only the writable ticket, or `F2` to reveal it.
   The page verifies that the ticket is writable and belongs to the entered
   workspace before importing it. It then starts synchronization and returns a
   server ticket.
5. Press Enter in the TUI and paste the server ticket returned by the page.
6. Press Enter again. The TUI validates that the returned ticket belongs to the
   active workspace, stores the peer relationship, and starts synchronization.

The successful screen displays the workspace ID and confirms that future TUI
and daemon launches will resume synchronization without either ticket. Press
Esc at any step to discard the in-memory invitation. Tickets and pasted server
output remain hidden unless `F2` is pressed. The setup page does not store the
operator token or either ticket in browser storage.

For headless recovery, press `C` in step 2 to copy the equivalent
`systemctl stop` / `xo-admin import-ticket` / `systemctl start` commands.

Never run `xo`, `xo-admin`, and `xo-syncd` concurrently against the same state
directory.

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
  been shared. Use device retirement or namespace rotation when a capability or
  device must be revoked.
- The binaries are not yet packaged by this repository; build or deploy the
  release binaries directly.
