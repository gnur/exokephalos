# xo

xo is an offline-first personal knowledge workspace. The `xo` terminal UI keeps
an ordinary Markdown projection on disk, while Iroh provides replicated state
and peer-to-peer synchronization. `xo-syncd` is an always-on peer: it stores a
copy of a workspace and gives intermittently connected TUI clients a stable peer
with which to synchronize.

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

## Set up a new synchronization server

### 1. Prepare a seed directory

`xo-admin import-workspace` creates the replicated workspace used by the
server. Its source can be an existing Markdown projection or an empty directory
for a new workspace.

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
```

Save both `workspace_id` and `ticket`. A writable ticket is a capability: anyone
who possesses it can join and write to the workspace. Transfer it privately and
do not commit it to a repository or put it in `config.scm`.

The state directory contains the server's endpoint identity, workspace records,
and blobs. Back it up and do not delete `endpoint.key`.

### 2. Start the daemon

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

## Connect the TUI

### 1. Create the local configuration

On the client machine:

```console
mkdir -p ~/.config/xo
xo config-init > ~/.config/xo/config.scm
```

The default configuration uses `~/.local/share/xo` for replicated local state
and `~/notes` for the Markdown projection:

```scheme
(xo-config
  (schema 1)
  (state-dir "~/.local/share/xo")
  (workspace #f)
  (projection "~/notes"))
```

### 2. Join with the server ticket

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

The TUI header reports connectivity, pending operations, missing blobs, and
convergence. Press `y` for detailed synchronization state and `r` to refresh and
retry synchronization.

### TUI navigation and tag filtering

Press `g` to open the goto menu. Every configured view and subview is shown with
its shortest unique prefix; type that prefix to switch immediately, or use the
arrow keys and Enter. View navigation does not use a command prompt or
separately configured direct-view keys.

Press `T` to show or hide the tag pane. When it is visible, `Tab` and
`Shift-Tab` include it in cyclic pane navigation. Use Left/Right or `h`/`l` for
spatial pane movement between Tags, Notes, and Preview. Highlight a tag with
Up/Down or `j`/`k`, then press Space or Enter to toggle that filter.

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
The source tree is never modified.

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
  (actions)
  (templates)
  (capability-grants))
```

Predicates support `always`, `field-equals`, `has-tag`, `not`, `all`, and
`any`. Actions use declarative effects such as `add-tag`, `remove-tag`,
`set-field`, and `append-body`; mutating actions require an explicit
`mutate-note` capability grant. Optional lexical modules below
`modules/**/*.scm` use the same fields inside `(workspace-module ...)`.

Only the native declarative form is accepted. Configuration is parsed through a
strict boundary: arbitrary filesystem, environment, process, network, clock,
or evaluation expressions are rejected.

## Add another client

Stop `xo-syncd` before running an administrative command against its state
directory, create another invitation, and then restart the daemon:

```console
sudo systemctl stop xo-syncd
sudo -u xo xo-admin invite /var/lib/xo-syncd '<WORKSPACE_ID>'
sudo systemctl start xo-syncd
```

Join on the new client using the printed ticket:

```console
xo --ticket '<NEW_TICKET>'
```

To create a client that can read but cannot publish writes, add `--read-only` to
the `xo-admin invite` command.

## Attach a server to an existing TUI workspace

If the workspace was created in the TUI, press `J` to open **Connect
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
