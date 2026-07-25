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

### Optional systemd service

Create a dedicated user and ensure it owns `/var/lib/xo-syncd`, then install a
unit such as `/etc/systemd/system/xo-syncd.service`:

```ini
[Unit]
Description=xo synchronization peer
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=xo
Group=xo
ExecStart=/usr/local/bin/xo-syncd --state-dir /var/lib/xo-syncd --operator-bind 127.0.0.1:9464
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/xo-syncd

[Install]
WantedBy=multi-user.target
```

Enable it with:

```console
sudo systemctl daemon-reload
sudo systemctl enable --now xo-syncd
sudo systemctl status xo-syncd
```

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

The current TUI does not yet restart Iroh's live-sync task from the stored peer
list. After restarting `xo`, pass the same server ticket again when live sync is
needed; importing an already-known capability is idempotent. The ticket is
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

## Add another client

Stop `xo-syncd` before running an administrative command against its state
directory, create another invitation, and then restart the daemon:

```console
sudo systemctl stop xo-syncd
xo-admin invite /var/lib/xo-syncd '<WORKSPACE_ID>'
sudo systemctl start xo-syncd
```

Join on the new client using the printed ticket:

```console
xo --ticket '<NEW_TICKET>'
```

To create a client that can read but cannot publish writes, add `--read-only` to
the `xo-admin invite` command.

## Attach a server to an existing TUI workspace

The cleanest setup is to initialize the workspace on the server first. If the
workspace already exists in a TUI state directory, the current administration
CLI does not yet have a headless `import-ticket` command. The supported
workaround is to import it once with `xo` on the server:

1. Stop the client TUI and obtain its workspace ID from
   `~/.local/share/xo/active-workspace`.
2. Create a writable invitation with `xo-admin invite` against the stopped
   client state directory.
3. On the server, initialize `~/.config/xo/config.scm`, then run:

   ```console
   xo \
     --state-dir /var/lib/xo-syncd \
     --projection /var/lib/xo-syncd-projection \
     --ticket '<CLIENT_TICKET>'
   ```

4. Allow the first synchronization to complete, press `q`, and start
   `xo-syncd` with `/var/lib/xo-syncd`.
5. With the daemon stopped, run `xo-admin invite` against the server state and
   use that new server-issued ticket once on the original client. This gives the
   client the server endpoint as a synchronization peer.

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
- Restarting the TUI reopens its active workspace but does not yet resume live
  sync automatically; pass the server ticket again to restart synchronization.
- The operator server is plain HTTP and binds to loopback by default. Keep it on
  loopback or place it behind a suitably secured reverse proxy.
- Ticket revocation is not equivalent to deleting a string that has already
  been shared. Use device retirement or namespace rotation when a capability or
  device must be revoked.
- The binaries are not yet packaged by this repository; build or deploy the
  release binaries directly.
