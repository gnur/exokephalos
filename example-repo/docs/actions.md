# Custom Actions

Custom actions are reusable modifiers configured in your configuration files that allow you to modify frontmatter using `yq` syntax. They are available in both the terminal user interface (TUI) and the web interface.

## Configuration

Actions are defined under the `[actions]` section in `.exo.toml` or in configuration files inside `.exo/` (e.g., `actions.toml`).

Each action configuration contains:
- `description`: The label describing the action.
- `filter`: Optional boolean CEL expression matching current tags or frontmatter to determine if the action is applicable to the selected item. If omitted, the action is always applicable.
- `expr`: A `yq` expression that modifies the frontmatter structure.

### Example Action Config

```toml
[actions.start-book]
description = "Start reading this book"
filter = '"to-read" in tags'
expr = '.tags -= ["to-read"] | .tags += ["reading"] | .started = now'

[actions.finish-book]
description = "Mark book as finished reading"
filter = '"reading" in tags'
expr = '.tags -= ["reading"] | .tags += ["read"] | .finished = now'

[actions.mark-done]
description = "Mark item as done"
filter = '"todo" in tags && !("done" in tags)'
expr = '.tags += ["done"]'
```

## How to Trigger Actions

### Terminal Interface (TUI)
1. Select an item in any view.
2. Press `:` to open the fuzzy action picker.
3. Type to filter by action name or description, use `Up`/`Down` to select, and press `Enter` to run the `yq` update.

Actions whose CEL filter does not match the selected item are shown grayed out. Selecting one shows the required CEL expression in the status line.

### Web Interface
When viewing an item details page (`/views/<type>/<id>`), any actions whose `filter` expression evaluates to true for the item will display as action buttons on the top right. Clicking an action button executes the update on disk.
