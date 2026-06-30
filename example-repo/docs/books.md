# Books & Goodreads Integration

exokephalos provides support for tracking books, importing book metadata from Goodreads, and visualizing reading statistics.

## Goodreads Import

To import book details from Goodreads:
1. Open the terminal user interface by running `exo`.
2. Select any view (like Books) and highlight an item or press `a` (or `Space`) to open the action menu.
3. In the action menu, press `i` to activate the **Import URL** prompt.
4. Enter or paste the Goodreads book URL (e.g., `https://www.goodreads.com/book/show/12345`) and press `Enter`.

The TUI will fetch the metadata from Goodreads and automatically create a new book file in your repository with all the metadata fields populated:
- Title
- Author(s) (as a list)
- Page count
- Cover image URL
- Goodreads URL

## Views & Stats Page

A view can be configured to display a summary page of reading statistics, including total pages read, books read per year, and progress charts.

### Enabling Stats in a View

To enable the stats page for a view, set the `stats_template` attribute to `"books/stats"` in your view configuration file:

```toml
[views.books]
name = "Books"
key = "b"
filter = '("read" in tags || "to-read" in tags || "reading" in tags || "stopped-reading" in tags)'
show_tags = false
title_field = "title"
subtitle_field = "author"
sort_field = "added"
sort_order = "desc"
stats_template = "books/stats"
```

Once `stats_template` is configured, a "Stats" option becomes available in the dropdown navigation of the web interface (at `/views/books/stats`) displaying reading charts.
