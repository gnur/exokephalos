# Web Interface

exokephalos includes a beautiful, premium, responsive web interface utilizing HTMX for fast, dynamic interactions.

## Starting the Web Server

To spin up the web interface, run the server subcommand:

```bash
EXO_DIR=/path/to/your/notes exo serve
```

By default, the server listens locally at:
`http://localhost:8293`

`EXO_DIR` must point at the data directory containing root-level workspace `*.toml` config files. If it is not set, exo uses `./example-repo`.

## Running With Docker

The web interface container image is published to GitHub Container Registry:

```bash
docker pull ghcr.io/gnur/exokephalos:latest
```

Mount your exo data directory at `/data` and publish port `8293`:

```bash
docker run --rm \
  -p 8293:8293 \
  -v "/path/to/your/notes:/data" \
  ghcr.io/gnur/exokephalos:latest
```

The image runs `exo serve` by default. Prebuilt binaries for TUI, web, and LSP usage are available from the [GitHub releases page](https://github.com/gnur/exokephalos/releases).

## Sync Server Mode

By default, `exo serve` uses local markdown files from `EXO_DIR`. To run it as a central sync server, create `.exo/serve.toml`:

```toml
[sync.server]
enabled = true
db_path = ".exo/server.sqlite"
listen = ":8293"
```

Then start the server:

```bash
EXO_DIR=/path/to/server-data exo serve
```

In sync-server mode, the server stores notes and synced root-level workspace config in SQLite. It does not scan or write markdown files. TUI clients keep markdown files locally and sync through signed HTTP requests and SSE updates.

New clients appear in the approval page:

```text
http://localhost:8293/admin/sync/clients
```

## Running The Sync Server On Kubernetes

Plain Kubernetes manifests are available in `deploy/kubernetes/`. They run exo as a single-replica `StatefulSet` with a `ReadWriteOnce` PVC mounted at `/data`, and they enable sync-server mode with `.exo/serve.toml`.

Apply the manifests:

```bash
kubectl apply -k deploy/kubernetes/
```

Wait for the pod and PVC:

```bash
kubectl rollout status statefulset/exo -n exo
kubectl get pvc -n exo
```

The default service is internal-only. Use port-forwarding for local access:

```bash
kubectl port-forward -n exo svc/exo 8293:8293
```

Then approve sync clients at `http://localhost:8293/admin/sync/clients`. The in-cluster service URL is `http://exo.exo.svc.cluster.local:8293`.

## Features

- **Dynamic Navigation**: Tabbed views and responsive layout loaded natively.
- **boosted page loads**: Powered by htmx for snappy navigation.
- **Creation & Modification**: Edit items in markdown, import URLs as notes, or trigger custom actions.
- **Metadata Cards**: The full YAML frontmatter of any item is rendered under the details page.
- **Reading Stats**: View stats graphs built with Chart.js.

## API Routes

The web server also exposes JSON API routes:

| Route | Description |
|-------|-------------|
| `POST /api/items` | Create a note from a URL |
| `GET /api/items/{id}` | Return an item as JSON with `frontmatter` and `body` |
| `PATCH /api/items/{id}` | Replace an item's `frontmatter` and/or `body` |
| `POST /api/query/ids` | Return sorted item IDs matching a CEL expression |

`POST /api/items` accepts a JSON object with a `url` field. The server fetches the page, extracts readable article HTML, converts it to markdown, and stores it as a `type: note` item.

```json
{"url":"https://example.com/article"}
```

`PATCH /api/items/{id}` accepts `frontmatter`, `body`, or both. Provided fields replace the complete stored value; omitted fields are preserved.

```json
{"frontmatter":{"id":"apibook","type":"book","title":"API Book"}}
```

`POST /api/query/ids` accepts a plain CEL expression in the request body. The expression uses the same CEL environment as views: `type`, `tags`, and `fm`.

```cel
type == "book" && "reading" in tags
```
