# Custom Actions

Custom actions are defined in the synced `exo.fnl` workspace configuration and run on the server for the web interface or on the local machine for the TUI.

```fennel
{:actions
 {:mark-done
  {:description "Mark item as done"
   :when (fn [note]
           (and (= note.type "note")
                (not= note.frontmatter.status "done")))
   :run (fn [note]
          (assoc note :frontmatter
                 (assoc note.frontmatter :status "done")))}}}
```

`:when` is optional and must return a boolean. `:run` receives the complete note (`:id`, `:path`, `:type`, `:tags`, `:frontmatter`, and `:body`) and must return its replacement. Actions can change both frontmatter and body.

Actions never receive unrestricted filesystem, shell, environment, or network access. They may request capabilities with `:permissions`; the executing host must grant the same action in local-only `.exo/permissions.fnl`. The browser never executes configuration code.

For example, an action that declares `:permissions [:filesystem :network]` can be granted only the paths and HTTPS origins needed on this machine:

```fennel
{:actions
 {:import-reference
  {:filesystem {:read ["references/*"]
                :write ["generated/*"]}
   :network {:origins ["https://openlibrary.org"]}}}}
```

The runtime exposes only `exo.filesystem.read`, `exo.filesystem.write`, and `exo.network.get` when their matching grant exists. Paths are workspace-relative and must match a grant; network requests are HTTPS-only, origin-checked, time-limited, redirect-checked, and response-limited.

Use `:` in the TUI to open the action picker. The web interface shows actions that match the current item.
