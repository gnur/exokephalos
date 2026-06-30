# Custom Actions

Custom actions are reusable modifiers configured in your configuration files that allow you to modify frontmatter using `yq` syntax. They are available in both the terminal user interface (TUI) and the web interface.

## Configuration

Actions are defined under the `[actions]` section in `.exo.toml` or in configuration files inside `.exo/` (e.g., `actions.toml`).

Each action configuration contains:
- `description`: The label describing the action.
- `filter`: A boolean filter expression matching current tags or frontmatter to determine if the action is applicable to the selected item.
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
2. Press `a` (or `Space`) to open the action menu.
3. Type the hotkey corresponding to the action (e.g., matching the action's description prefix or name) and press `Enter` to run the `yq` update.

### Web Interface
When viewing an item details page (`/views/<type>/<id>`), any actions whose `filter` expression evaluates to true for the item will display as action buttons on the top right. Clicking an action button executes the update on disk.
