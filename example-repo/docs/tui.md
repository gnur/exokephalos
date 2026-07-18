# Terminal User Interface (TUI)

xo features an interactive terminal user interface built on Bubble Tea, providing full parity with the storage repository.

## Starting the TUI

To start the TUI mode, run the `xo` command with no arguments:

```bash
EXO_DIR=/path/to/your/notes xo
```

`EXO_DIR` must point at the data directory containing root-level workspace `*.toml` config files. If it is not set, exo uses `./example-repo`.

## Keybindings

The TUI uses intuitive, single-key commands:

| Key | Action |
| --- | --- |
| `n` | Create a new item (interactive wizard using templates) |
| `e` / `Enter` | Open the item in your editor (`$EDITOR`) |
| `d` / `Delete` | Delete the currently selected item |
| `:` | Open fuzzy action picker |
| `Tab` | Cycle forwards through subview tabs |
| `Shift + Tab` | Cycle backwards through subview tabs |
| `j` / `Down` | Move selection down |
| `k` / `Up` | Move selection up |
| `/` | Focus search bar to filter list |
| `Esc` | Clear search or close modals |
| `q` / `Ctrl + C` | Exit the TUI |

The action picker includes configured actions plus built-ins like Goodreads import, Hardcover search, URL-to-note import, and sync actions when sync is configured. Select `url-to-note`, paste a web page URL, and exo will extract the readable HTML, convert it to markdown, and create a `type: note` item. Actions whose CEL filter does not match are grayed out; selecting one shows the required CEL expression.

## Sync Actions

When `.exo/tui.toml` contains a `[sync]` section, the TUI adds sync actions:

```toml
[sync]
server_url = "http://localhost:8293"
client_id = "laptop"
```

Use `start-sync` to generate a local ed25519 keypair and register this client with `xo serve`. Approve the client in the web UI under the `sync clients` tab. The TUI keeps polling while it waits; after approval it continues automatically and pushes local markdown files plus root-level workspace config. You do not need to run `start-sync` a second time.

Use `sync-outbox` to inspect pending, failed, and synced operations. The outbox view supports scrolling, status filtering, entry details, retrying one selected entry, and retrying all failed entries. If the server is offline, local edits continue writing to markdown and are retried from the outbox later. After approval, the TUI reconciles every 5 seconds and the footer shows the current sync state when sync is configured.

An `All` view is always available with key `0`; it shows every item regardless of type.
