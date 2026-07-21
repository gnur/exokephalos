# Books & Metadata Integrations

exokephalos provides support for tracking books, importing book metadata from Goodreads or Hardcover, and visualizing reading statistics.

## Hardcover Search

Set `HARDCOVER_TOKEN` in the environment before launching the TUI.

To add a book from Hardcover:
1. Open the terminal user interface by running `EXO_DIR=/path/to/your/notes xo`.
2. Press `:` to open the action picker and select `hardcover-search`.
3. Enter a search query and press `Enter`.
4. Pick one of the top five results by pressing `1` through `5`.

The selected result is converted into a new `type: book` item using the configured books view template.

## Goodreads Import

To import book details from Goodreads:
1. Open the terminal user interface by running `EXO_DIR=/path/to/your/notes xo`.
2. Press `:` to open the action picker.
3. Select `goodreads-import` to activate the **Import URL** prompt.
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

To enable the stats page for a view, set `:stats-template` to `"books/stats"` in `exo.fnl`:

```fennel
{:views {:books {:name "Books" :key "b" :show-tags false
                 :title-field "title" :subtitle-field "author"
                 :sort-field "added" :sort-order "desc"
                 :when (fn [note] (= note.type "book"))
                 :stats-template "books/stats"}}}
```

Once `stats_template` is configured, a "Stats" option becomes available in the dropdown navigation of the web interface (at `/views/books/stats`) displaying reading charts.
