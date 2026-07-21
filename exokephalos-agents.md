# exokephalos

exokephalos is a personal knowledge manager. Notes are Markdown files with YAML frontmatter; the filesystem is the source of truth. `xo` opens the TUI, `xo serve` runs the web and sync server, and `xo lsp` starts the editor language server.

Set `EXO_DIR` to a workspace directory. It must contain a root-level `exo.fnl`; optional `modules/**/*.fnl` and `modules/**/*.lua` files are synced workspace configuration. `.exo/` is local-only state and machine configuration.

## Workspace views and actions

`exo.fnl` is Fennel. Views select notes with `:when`; predicates receive a note with fields such as `type`, `tags`, `frontmatter`, and `body`.

```fennel
{:default-view :notes
 :views
 {:notes {:name "Notes" :key "n" :show-tags true
          :when (fn [note] (= note.type "note"))
          :subviews [{:name "All" :when (fn [_] true)}]}
  :books {:name "Books" :key "b"
          :when (fn [note] (= note.type "book"))}}
 :actions
 {:append-marker {:description "Append marker"
                  :when (fn [note] (= note.type "note"))
                  :run (fn [note] (assoc note :body (.. note.body "\nDone")))}}}
```

Views require `:name`, `:key`, and a function `:when`. New items use the shared type/title/body flow; views do not define item templates. Optional display settings include `:title-field`, `:subtitle-field`, `:sort-field`, `:sort-order`, `:show-tags`, `:preview-template`, `:stats-template`, and `:subviews`.

## Web server

The server stores synced items and workspace configuration in SQLite. Configure it locally in `EXO_DIR/.exo/serve.fnl`:

```fennel
{:server {:db-path ".exo/server.sqlite"
          :listen ":8293"}}
```

Run it with `EXO_DIR=/path/to/server xo serve`. The database path is relative to `EXO_DIR` unless absolute.

## TUI and sync client

Configure a TUI client locally in `EXO_DIR/.exo/tui.fnl`:

```fennel
{:sync {:server-url "http://localhost:8293"
        :client-id "laptop"}}
```

Run `EXO_DIR=/path/to/client xo`, open the action picker with `:`, and run `start-sync`. The client generates a local signing key and must be approved in the web UI before it can sync.
