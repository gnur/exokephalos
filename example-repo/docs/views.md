# View Configurations

Views are custom collections that define how different markdown files in your repository are queried, displayed, and filtered.

exo also adds a built-in `All` view with key `0`. This view uses the filter `true`, so it shows every item regardless of type in both the TUI and web interface at `/views/all`.

## Configuring Views

Views are configured in your `.exo.toml` file under `[views.<view_id>]`.

### Config Parameters

- `name`: The display label shown in the navigation menus.
- `type`: The `type` field value in markdown frontmatter that matches this view.
- `title_field` (optional): The frontmatter field to use as the title in lists (defaults to `title`).
- `subtitle_field` (optional): The frontmatter field to use as the subtitle in lists.
- `stats_template` (optional): Set to `"books"` to enable reading statistics.
- `subviews` (optional): Tabbed filters inside the view.

### Example Configuration

```toml
[views.notes]
name = "Notes"
type = "note"
title_field = "title"

[[views.notes.subviews]]
name = "All"
filter = ""

[[views.notes.subviews]]
name = "Todo"
filter = "tag:todo"
```

## Subview Filters

Filters match tags using query strings:
- `tag:<tagname>` matches items containing the specified tag.
- Multiple filters can be combined, or left blank (`""`) to match all items of the given type.
