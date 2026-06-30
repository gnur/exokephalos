# Improvements

Code review findings for Exokephalos, updated after the generic item refactoring.

---

## P1 — Security

All P1 security issues have been resolved.

## P2 — Code Duplication

All P2 code duplication issues have been resolved by consolidating them into shared utilities under `internal/markdown`, `internal/id`, `internal/scanner`, and `internal/lsp/notes.go`.

## P3 — Error Handling (ignored errors)

All P3 error-handling issues have been resolved.

## P4 — Performance

All P4 performance issues have been resolved.

## P5 — Missing Tests

| Package | Notes |
|---------|-------|
| `internal/markdown/` | Foundational package, zero tests |
| `internal/handlers/` | Largest package, zero unit tests (only integration tests) |
| `internal/goodreads/` | Zero tests |

## P6 — Legacy / Dead Code

| # | Issue | Location |
|---|-------|----------|
| 24 | `scanner.ScanAll()` is superseded by `cache.scan()` | `internal/scanner/scanner.go` (unused dead code) |

## P7 — Other Improvements

| # | Issue | Location |
|---|-------|----------|
| 26 | **`tui.go` is a 1190-line god file** — split into `keys.go`, `filtering.go`, `actions.go` | `internal/tui/tui.go` |
| 27 | **Inconsistent date formats** — TUI uses `time.RFC3339`, web uses `2006-01-02T15:04:05` for DateTime autofill | `internal/tui/create.go` vs `internal/handlers/generic.go` |
| 28 | **No config validation for `sort_order`** — accepts any string, should validate `asc`/`desc` | `internal/config/config.go` |
| 29 | **CEL filter syntax not validated at config load time** — only fails at runtime | `internal/config/config.go` |
| 30 | **Webhook filenames use colons** (`source-14:30:00.md`) — breaks on Windows filesystems | `internal/handlers/legacy.go:45` |
| 31 | **No HTTP timeout on Goodreads client** — uses `http.DefaultClient` | `internal/goodreads/goodreads.go:50` |
| 32 | **LSP `resolveStrikethrough`, `resolveMarkDone`, `resolveBookStatus` are near-identical** — consolidate into one function | `internal/lsp/codeaction.go:259-341` |
| 33 | **TUI action menu matches by first char only** — collisions if two actions share a first letter; no collision detection | `internal/tui/tui.go:286` |
| 34 | **Markdown parser doesn't handle UTF-8 BOM** — files with BOM won't be parsed | `internal/markdown/parser.go` |
| 35 | **`id.go` `randomChars` uses predictable `math/rand`** — should use `crypto/rand` | `internal/id/id.go:39` |
| 37 | **`WebhookReceive` reads entire body into memory** — no size limit (duplicate of #9) | `internal/handlers/legacy.go` |
| 39 | **LSP `DidChange` type assertion is fragile** — `.(map[string]any)` depends on JSON deserialization behavior | `internal/lsp/server.go:96` |
| 40 | **LSP `addTagToLine`/`removeTagFromLine` are fragile string manipulators** — could break on edge cases (substring tags, tags in comments) | `internal/lsp/codeaction.go:343-370` |
| 41 | **`ViewEdit` POST does not validate content** — raw form data written directly to disk | `internal/handlers/generic.go:160` |
| 42 | **TUI `extractTemplateVars` is fragile** — uses simple string matching instead of template parsing; can produce false positives | `internal/handlers/generic.go:449` |
| 43 | **`statsBuilders` map is hardcoded** — adding new stats requires code changes instead of configuration | `internal/handlers/stats.go` |
| 44 | **TUI `importBook` finds books view by name substring** — `strings.Contains(name, "book")` matches "notebook" | `internal/tui/tui.go:972` |
| 45 | **TUI model uses value receiver on `Update()`** — copies entire state (all items, view states) on every bubbletea message | `internal/tui/tui.go` |

---

## Resolved / Completed Improvements

These issues have been resolved as part of the generic item refactoring or other updates:

* **7: XSS via markdown template function**: (Resolved - goldmark output is now sanitized using `bluemonday.UGCPolicy()`)
* **8: SSRF in Goodreads import**: (Resolved - `FetchBook` validates URL schemes and hosts to be goodreads.com domains)
* **9: No request body size limits**: (Resolved - `WebhookReceive` restricts incoming body to 2MB using `http.MaxBytesReader`)
* **10: Path traversal in `ViewNewPost`**: (Resolved - validated target path to guarantee it stays inside `baseDir` for both TUI and web create templates)
* **11: No CSRF protection**: (Resolved - implemented `CSRFMiddleware` validating Origin, Referer, and Sec-Fetch-Site headers on all form POSTs)
* **30: Webhook filenames use colons**: (Resolved - webhook filenames now use hyphens to support Windows filesystems)
* **31: No HTTP timeout on Goodreads client**: (Resolved - Goodreads client now uses a 10-second timeout)
* **P3: Error Handling**: (Resolved - directory walking, fsnotify watcher, frontmatter parsing, and CEL evaluation errors are now properly logged and handled)
* **P4: Performance**: (Resolved - optimized LSP lookups via O(1) ID caching, cached tag counting in TUI, precompiled preview templates, and short-circuited TUI body search)
* **P2: Code Duplication**: (Resolved - consolidated tag extraction, frontmatter parsing, ID verification, slugification, folder skipping, test helpers, and LSP note lookups)
* **P0 Bugs**: Fix P0 bugs first, including data loss on UpdateBook and LSP race. (Resolved)
* **1: Data loss on UpdateBook**: (Resolved - item handlers are now generic and preserve fields correctly)
* **4: LSP race condition**: (Resolved)
* **`getStringSlice`**: (Resolved - specific typed scans/models are gone)
* **12: `os.MkdirAll` error ignored**: (Resolved - generic `CreateItem` handles/returns errors correctly)
* **18: `repo.GetBook` calls `ListBooks` 4x**: (Resolved - specific typed methods are gone)
* **22: `repo.Get*` methods are O(n) linear scans**: (Resolved - data layer has been completely refactored to generic methods using the cache or direct path access)
* **23: `models/` structs are partially superseded by `scanner.Item`**: (Resolved - `models/` package has been entirely deleted)
* **25: `handlers/legacy.go` special-purpose handlers overlap**: (Resolved - Goodreads book import has been deleted from handlers, legacy.go now only handles webhook ingestion)
* **36: `MoveBook` is not atomic**: (Resolved - moving books is no longer an active codebase path/operation)
* **46: Benchmark test creates 2000 files on disk but never cleans up**: (Resolved - benchmark test uses `defer os.RemoveAll(tmpDir)`)
* **47: `repo.ListNotes` depth limit**: (Resolved - method replaced by generic ListItems querying prefix index in cache)
* **`internal/repo/` missing tests**: (Resolved - `repo_test.go` has been implemented)

---

## Suggested Execution Order

1. **Address P1 security** — at minimum #7 (XSS), #8 (SSRF), and #9 (Request limits) since the web interface is network-accessible.
2. **Extract shared utilities (P2)** — consolidate `extractTags`, `fmString`, and `slugify`/`ensureID` helpers to reduce code duplication and eliminate bugs.
3. **Fix remaining error handling (P3)** — quick wins in the file watcher and cache.
4. **Add tests for `markdown/` and `handlers/` (P5)** — foundational packages where changes cascade.
5. **Performance (P4)** and **cleanup (P6, P7)** can be done incrementally.
