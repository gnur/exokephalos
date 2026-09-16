use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, StatusCode};

const DOCUMENT: &str = r###"# Exokephalos (xo)

Base URL for this xo instance: {{BASE_URL}}

## Purpose

Exokephalos is an offline-first personal knowledge workspace. Notes are typed Markdown items with structured frontmatter. Native and browser clients keep durable local Automerge replicas, continue working offline, and synchronize through one self-hosted xo-syncd server. Immutable revisions preserve concurrent edits and conflicts instead of silently overwriting them.

The project consists of:

- `xo`: terminal UI, local Markdown projection, import/export, and local Steel plugin manager.
- `xo-syncd`: synchronization server, authenticated HTTP API, webhook receiver, and PWA host.
- `xo-pwa`: installable browser client embedded in xo-syncd releases.
- `xo-lsp`: diagnostics and completion for projected Markdown files.

A typical item has YAML frontmatter and a Markdown body:

```markdown
---
id: abc2345
type: note
title: Example
tags: [documentation]
created: 2026-09-14T12:00:00+00:00
---
The item body.
```

The Markdown directory used by the native client is a projection, not the synchronization transport or a complete backup.

## Inline Steel in notes

A note body may contain a fenced `steel` block. xo evaluates the block for preview in the TUI and PWA; the generated output is not saved into the note. The final Steel value must be a Markdown string. Evaluation replaces only that fenced block and retains the surrounding body. A failure renders as `> Steel block error: <message>` in place of the failed block.

````markdown
```steel
(let ([books (string->jsexpr (xo-query "{\"type\":\"book\"}"))])
  (string-append "## Reading stats\\nBooks: "
                 (number->string (length books))))
```
````

Two read-only xo host functions are available:

- `(current-note-id)` returns the ID string of the note containing the block. Use it instead of hardcoding an ID so blocks remain portable when copied or imported.
- `(xo-query filter-json)` accepts a JSON object and returns a JSON array of matching `{id, frontmatter, body}` objects. Use `{}` to query all non-deleted items.

`xo-query` applies a conjunction of top-level exact-equality comparisons against frontmatter. It does not support nested property paths, partial matches, ordering, or operators such as `<` and `>`. Extract nested values and perform comparisons, filtering, date handling, and aggregation in Steel after querying.

Query the current note without embedding its ID:

```scheme
(define self
  (car (string->jsexpr
    (xo-query
      (value->jsexpr-string (hash 'id (current-note-id)))))))
(define self-frontmatter (hash-ref self 'frontmatter))
(define self-created (hash-ref self-frontmatter 'created))
```

`string->jsexpr` converts JSON objects to Steel hash tables (`hash?`) and JSON arrays to Scheme lists (`list?`). JSON object keys become symbols, not strings: use `(hash-ref item 'id)` and `(hash-contains? item 'frontmatter)`. Process JSON arrays with list operations such as `car`, `cdr`, `map`, `filter`, and `length`.

The normal `Engine::new_sandboxed()` Steel core is available. Common useful functions include arithmetic and comparisons, `quotient`, `modulo`, `substring`, `string-append`, `string-length`, `number->string`, `string->number`, `inexact->exact`, `map`, and `filter`. Racket-specific libraries, filesystem module loading, external crates, and host functions not listed here are unavailable; keep blocks self-contained.

There is no clock. For time-based filtering, use the current note's `created` value or derive a relative reference point from the newest queried entry. Compare only consistently normalized date strings (for example a fixed-offset `YYYY-MM-DD` prefix), or store a numeric/sortable timestamp field in frontmatter. A block cannot know the actual current time.

Each block runs in a fresh sandbox. It cannot mutate notes or access the filesystem, environment, network, processes, terminal, or clock. Block source is limited to 64 KiB, output to 256 KiB, and execution is time-bounded.

## Steel configuration

The native client is configured in `~/.config/xo/config.scm`:

```scheme
(xo-config
  (schema 5)
  (state-dir "~/.local/share/xo")
  (client-id #f)
  (server "{{BASE_URL}}")
  (projection "~/notes"))
```

xo-syncd is normally configured in `~/.config/xo-syncd/config.scm`:

```scheme
(xo-syncd-config
  (schema 1)
  (state-dir "~/.local/share/xo-syncd")
  (bind "127.0.0.1:9464")
  (oidc-issuer "https://id.example.com")
  (oidc-audience "{{BASE_URL}}")
  (oidc-client-id "PUBLIC_OIDC_CLIENT_ID"))
```

Workspace behavior is also Steel data and is replicated to clients. Edit it with the `edit_workspace_config` TUI action. A workspace configuration declares views, subviews, predicates, actions, templates, query limits, and explicit capability grants. Predicates include `always`, `field-equals`, `has-tag`, `not`, `all`, and `any`. Mutation effects include `add-tag`, `remove-tag`, `set-field`, and `append-body`.

```scheme
(workspace-config
  (schema 1)
  (default-view "notes")
  (query-limit 500)
  (views
    (view
      (id "notes")
      (name "Notes")
      (key "n")
      (show-tags #t)
      (title-field "title")
      (subtitle-field #f)
      (sort-field "created")
      (descending #t)
      (preview #f)
      (predicate (field-equals "type" "note"))
      (subviews)))
  (actions
    (action
      (id "mark-done")
      (description "Mark item as done")
      (predicate (has-tag "todo"))
      (effects (add-tag "done"))))
  (templates)
  (capability-grants
    (grant
      (action "mark-done")
      (capabilities mutate-note))))
```

Local executable Steel plugins are stored under `~/.config/xo/plugins/`. Unlike workspace configuration, plugins are not synchronized.

## xo commands

Running `xo` with no subcommand opens the terminal UI.

```text
xo [--state-dir PATH] [--client-id ID] [--server URL] [--projection PATH]
xo config-init
xo keymap-init
xo validate PATH
xo import SOURCE [--type TYPE]
xo export DESTINATION [--type TYPE]
xo plugin list
xo plugin install NAME SOURCE
xo plugin update NAME SOURCE
xo plugin remove NAME
```

- `config-init`: print a default native Steel configuration.
- `keymap-init`: print the default Steel keymap.
- `validate`: validate one Markdown document.
- `import`: recursively validate and import a copy of a Markdown tree.
- `export`: export current winning revisions as conventional Markdown.
- `plugin`: list, install, replace, or remove local executable Steel plugins. Use `-` as an install/update source to read from standard input.

## HTTP API

JSON request bodies are limited to 1 MiB. Use `Authorization: Bearer TOKEN`. API keys can grant `xo:read`, `xo:write`, and `xo:sync`. API-key management itself requires an OAuth access token.

### Public endpoints

- `GET {{BASE_URL}}/healthz`: returns `ok`.
- `GET {{BASE_URL}}/.well-known/xo-configuration`: returns non-secret OIDC client settings.
- `GET {{BASE_URL}}/llm.txt`: returns this URL-aware document.
- `POST {{BASE_URL}}/api/webhook/{source}`: creates a webhook item from headers and a JSON or text payload. This endpoint is unauthenticated; apply reverse-proxy rate and body limits.
- Other GET routes serve the embedded PWA with SPA fallback.

```console
curl -X POST '{{BASE_URL}}/api/webhook/github' \
  -H 'Content-Type: application/json' \
  --data '{"event":"created","repository":"example"}'
```

### Item endpoints

- `GET /api/items/{id}` (`xo:read`): read an item as `{"frontmatter":{},"body":["segment"]}`.
- `POST /api/items` (`xo:write`): capture a public URL after SSRF-safe DNS and redirect validation.
- `POST /api/item/{type}` (`xo:write`): create a typed item from plain text or JSON.
- `PATCH /api/items/{id}` (`xo:write`): create an updated immutable revision.
- `DELETE /api/items/{id}` (`xo:write`): create a deleted revision.

Capture a URL:

```console
curl -X POST '{{BASE_URL}}/api/items' \
  -H 'Authorization: Bearer TOKEN' \
  -H 'Content-Type: application/json' \
  --data '{"url":"https://example.com/article"}'
```

Create a typed item from UTF-8 plain text:

```console
curl -X POST '{{BASE_URL}}/api/item/note' \
  -H 'Authorization: Bearer TOKEN' \
  -H 'Content-Type: text/plain; charset=utf-8' \
  --data-binary 'A plain Markdown note.'
```

Create a typed item from JSON. `body` must be a string and `frontmatter` is optional. The server generates and overrides `id` and `created`. The `{type}` in the URL always overrides a conflicting `frontmatter.type`.

```console
curl -X POST '{{BASE_URL}}/api/item/task' \
  -H 'Authorization: Bearer TOKEN' \
  -H 'Content-Type: application/json' \
  --data '{"frontmatter":{"title":"Ship release","type":"note","tags":["todo"]},"body":"Run the release checklist."}'
```

A successful create returns HTTP 201 with the generated `id`, complete `frontmatter`, and `body`. Supported item-creation media types are `text/plain` and `application/json`. Invalid types or JSON return 400, unsupported media types return 415, and oversized bodies return 413.

Update an item with the object form:

```console
curl -X PATCH '{{BASE_URL}}/api/items/ITEM_ID' \
  -H 'Authorization: Bearer TOKEN' \
  -H 'Content-Type: application/json' \
  --data '{"frontmatter":{"title":"Replacement metadata"},"body":"Replacement body"}'
```

The object form replaces each supplied field as a whole. It may omit either `frontmatter` or `body`.

Update with RFC 6902 JSON Patch. The body is represented as an array of newline-joined text segments for patching:

```console
curl -X PATCH '{{BASE_URL}}/api/items/ITEM_ID' \
  -H 'Authorization: Bearer TOKEN' \
  -H 'Content-Type: application/json-patch+json' \
  --data '[
    {"op":"add","path":"/body/0","value":"Prepended paragraph"},
    {"op":"add","path":"/body/-","value":"Appended paragraph"}
  ]'
```

### Synchronization

- `GET /api/sync`: WebSocket synchronization endpoint requiring all of `xo:read`, `xo:write`, and `xo:sync`. It uses the `xo-sync` WebSocket subprotocol and xo's versioned Automerge protocol; it is not a general JSON WebSocket API.

### API-key management

These routes require an OAuth access token and cannot be called with a personal API key:

- `GET /api/api-keys`: list the authenticated user's API keys.
- `POST /api/api-keys`: create a scoped API key. The secret is returned once.
- `DELETE /api/api-keys/{id}`: revoke an API key.

```console
curl -X POST '{{BASE_URL}}/api/api-keys' \
  -H 'Authorization: Bearer OAUTH_ACCESS_TOKEN' \
  -H 'Content-Type: application/json' \
  --data '{"label":"automation","permissions":["xo:read","xo:write"]}'
```

For source, installation instructions, and fuller plugin documentation, see https://github.com/gnur/exokephalos.
"###;

pub fn serve<B>(request: &Request<B>) -> Response<Full<Bytes>> {
    let document = document(request);
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/plain; charset=utf-8")
        .header("cache-control", "no-cache")
        .header("x-content-type-options", "nosniff")
        .body(Full::new(Bytes::from(document)))
        .expect("llm.txt response headers are valid")
}

fn document<B>(request: &Request<B>) -> String {
    DOCUMENT.replace("{{BASE_URL}}", &request_origin(request))
}

fn request_origin<B>(request: &Request<B>) -> String {
    let scheme = first_header_value(request, "x-forwarded-proto")
        .filter(|value| matches!(*value, "http" | "https"))
        .unwrap_or("http");
    let valid_host = |value: &&str| {
        !value.is_empty()
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
            })
    };
    let host = first_header_value(request, "x-forwarded-host")
        .filter(valid_host)
        .or_else(|| first_header_value(request, "host").filter(valid_host))
        .unwrap_or("localhost");
    format!("{scheme}://{host}")
}

fn first_header_value<'a, B>(request: &'a Request<B>, name: &str) -> Option<&'a str> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_uses_forwarded_public_origin() {
        let request = Request::builder()
            .header("host", "127.0.0.1:9464")
            .header("x-forwarded-proto", "https")
            .header("x-forwarded-host", "notes.example.test")
            .body(())
            .unwrap();
        let document = document(&request);
        assert!(document.contains("Base URL for this xo instance: https://notes.example.test"));
        assert!(document.contains("https://notes.example.test/api/item/task"));
        assert!(document.contains("(current-note-id)"));
        assert!(document.contains("JSON object keys become symbols"));
        assert!(!document.contains("{{BASE_URL}}"));
    }

    #[test]
    fn unsafe_forwarded_values_are_not_reflected() {
        let request = Request::builder()
            .header("host", "localhost:9464")
            .header("x-forwarded-proto", "javascript")
            .header("x-forwarded-host", "example.test/path")
            .body(())
            .unwrap();
        assert!(
            document(&request).contains("Base URL for this xo instance: http://localhost:9464")
        );
    }
}
