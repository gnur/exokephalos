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

The daemon uses Iroh for synchronization. The operator address is only for
health, status, and Prometheus metrics; port `9464` is not the synchronization
port and does not need to be exposed to TUI clients.

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

The TUI pairing wizard described below generates commands for the system
service and `/var/lib/xo-syncd`. A user-service installation can use the same
flow by running the equivalent `systemctl --user` and non-`sudo` commands.

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
`Shift-Tab` include it in pane navigation. Highlight a tag with the arrow keys
or `j`/`k`, then press Space or Enter to toggle that filter.

Tag counts are live facets. They first respect the active view or subview and
the `/` title query, then show how many notes would remain if each tag were
added to the currently selected tag filters. Selecting or clearing a tag
therefore updates every displayed count immediately.

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
3. Press `c` to copy the generated stop/import/start commands using the
   terminal's OSC 52 clipboard support. Press `F2` if the terminal does not
   support clipboard writes and the commands need to be displayed for manual
   copying.
4. Run the commands on the server. They stop the system service, import the
   workspace as the `xo` service user, and restart `xo-syncd`.
5. Press Enter in the TUI and paste either the complete `xo-admin` output or
   only its `ticket=...` line.
6. Press Enter again. The TUI validates that the returned ticket belongs to the
   active workspace, stores the peer relationship, and starts synchronization.

The successful screen displays the workspace ID and confirms that future TUI
and daemon launches will resume synchronization without either ticket. Press
Esc at any step to discard the in-memory invitation. Tickets and pasted server
output remain hidden unless `F2` is pressed.

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

- `xo-syncd` hosts workspaces already present in its state directory; it does
  not currently create or import workspaces through the operator HTTP API.
- The operator server is plain HTTP and binds to loopback by default. Keep it on
  loopback or place it behind a suitably secured reverse proxy.
- Ticket revocation is not equivalent to deleting a string that has already
  been shared. Use device retirement or namespace rotation when a capability or
  device must be revoked.
- The binaries are not yet packaged by this repository; build or deploy the
  release binaries directly.
