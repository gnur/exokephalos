use std::convert::Infallible;
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper::body::{Body as HttpBody, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Role;
use xo_core::NoteId;
use xo_core::domain::Frontmatter;

use crate::central::CentralWorkspace;

type Body = Full<Bytes>;
const MAX_API_BODY_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchItem {
    frontmatter: Option<Frontmatter>,
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateItem {
    url: String,
}

#[derive(Debug, Serialize)]
struct ItemResponse {
    frontmatter: Frontmatter,
    body: String,
}

pub async fn serve(
    listener: TcpListener,
    workspace: Arc<CentralWorkspace>,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = &mut shutdown => return Ok(()),
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let workspace = Arc::clone(&workspace);
                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let workspace = Arc::clone(&workspace);
                        async move { Ok::<_, Infallible>(handle(request, workspace).await) }
                    });
                    if let Err(error) = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .with_upgrades()
                        .await
                    {
                        eprintln!("xo-syncd connection failed: {error}");
                    }
                });
            }
        }
    }
}

async fn handle(
    mut request: Request<Incoming>,
    workspace: Arc<CentralWorkspace>,
) -> Response<Body> {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    match (&method, path.as_str()) {
        (&Method::GET, "/healthz") => response(StatusCode::OK, "text/plain; charset=utf-8", "ok\n"),
        (&Method::GET, "/api/sync") => websocket_upgrade(&mut request, workspace),
        (&Method::POST, "/api/items") => create_item(request, &workspace).await,
        (method, path) if path.starts_with("/api/items/") => {
            item_request(method, path, request, &workspace).await
        }
        _ if path.starts_with("/api/") => json_error(StatusCode::NOT_FOUND, "not found"),
        _ => response(
            StatusCode::SERVICE_UNAVAILABLE,
            "text/plain; charset=utf-8",
            "PWA assets are not embedded in this migration build\n",
        ),
    }
}

async fn create_item(request: Request<Incoming>, workspace: &CentralWorkspace) -> Response<Body> {
    let create = match parse_json::<CreateItem>(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let page = match xo_core::url_capture::UrlCaptureService::default()
        .capture(&create.url)
        .await
    {
        Ok(page) => page,
        Err(error) => return json_error(StatusCode::UNPROCESSABLE_ENTITY, &error.to_string()),
    };
    let note = match xo_core::url_capture::captured_note(page, time::OffsetDateTime::now_utc()) {
        Ok(note) => note,
        Err(error) => return internal_error(&error),
    };
    match workspace.create_item(&note).await {
        Ok(true) => json_response(
            StatusCode::CREATED,
            &serde_json::json!({
                "id": note.id,
                "frontmatter": note.frontmatter,
                "body": note.body,
            }),
        ),
        Ok(false) => json_error(StatusCode::CONFLICT, "generated item already exists"),
        Err(error) => internal_error(&error),
    }
}

async fn item_request(
    method: &Method,
    path: &str,
    request: Request<Incoming>,
    workspace: &CentralWorkspace,
) -> Response<Body> {
    let id = &path["/api/items/".len()..];
    if id.is_empty() || id.contains('/') || !xo_core::id::is_valid(id) {
        return json_error(StatusCode::BAD_REQUEST, "invalid item id");
    }
    let note_id = NoteId::new(id);
    match *method {
        Method::GET => match workspace.item(&note_id).await {
            Ok(Some(note)) => json_response(
                StatusCode::OK,
                &ItemResponse {
                    frontmatter: note.frontmatter,
                    body: note.body,
                },
            ),
            Ok(None) => json_error(StatusCode::NOT_FOUND, "item not found"),
            Err(error) => internal_error(&error),
        },
        Method::PATCH => {
            let update = match parse_json::<PatchItem>(request).await {
                Ok(value) => value,
                Err(response) => return response,
            };
            match workspace
                .patch_item(&note_id, update.frontmatter, update.body)
                .await
            {
                Ok(Some(note)) => json_response(
                    StatusCode::OK,
                    &ItemResponse {
                        frontmatter: note.frontmatter,
                        body: note.body,
                    },
                ),
                Ok(None) => json_error(StatusCode::NOT_FOUND, "item not found"),
                Err(error) if error.to_string().contains("frontmatter id") => {
                    json_error(StatusCode::CONFLICT, &error.to_string())
                }
                Err(error) => internal_error(&error),
            }
        }
        Method::DELETE => match workspace.delete_item(&note_id).await {
            Ok(true) => response(StatusCode::NO_CONTENT, "application/json", Bytes::new()),
            Ok(false) => json_error(StatusCode::NOT_FOUND, "item not found"),
            Err(error) => internal_error(&error),
        },
        _ => json_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
    }
}

async fn parse_json<T: serde::de::DeserializeOwned>(
    request: Request<Incoming>,
) -> Result<T, Response<Body>> {
    if request
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_none_or(|value| !value.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err(json_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "content-type must be application/json",
        ));
    }
    if request
        .body()
        .size_hint()
        .upper()
        .is_some_and(|size| size > MAX_API_BODY_BYTES)
    {
        return Err(json_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body too large",
        ));
    }
    let body = request
        .into_body()
        .collect()
        .await
        .map_err(|_| json_error(StatusCode::BAD_REQUEST, "could not read request body"))?
        .to_bytes();
    if body.len() as u64 > MAX_API_BODY_BYTES {
        return Err(json_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body too large",
        ));
    }
    serde_json::from_slice(&body)
        .map_err(|_| json_error(StatusCode::BAD_REQUEST, "invalid JSON body"))
}

fn json_response(status: StatusCode, value: &impl Serialize) -> Response<Body> {
    match serde_json::to_vec(value) {
        Ok(body) => response(status, "application/json", body),
        Err(error) => internal_error(&error),
    }
}

fn json_error(status: StatusCode, message: &str) -> Response<Body> {
    json_response(status, &serde_json::json!({ "error": message }))
}

fn internal_error(error: &(impl std::fmt::Display + ?Sized)) -> Response<Body> {
    eprintln!("xo-syncd API request failed: {error:#}");
    json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
}

fn websocket_upgrade(
    request: &mut Request<Incoming>,
    workspace: Arc<CentralWorkspace>,
) -> Response<Body> {
    let Some(key) = request
        .headers()
        .get("sec-websocket-key")
        .map(|value| value.as_bytes().to_vec())
    else {
        return response(
            StatusCode::BAD_REQUEST,
            "text/plain; charset=utf-8",
            "WebSocket upgrade required\n",
        );
    };
    let version_matches = request
        .headers()
        .get("sec-websocket-version")
        .is_some_and(|value| value == "13");
    let upgrade_matches = request
        .headers()
        .get("upgrade")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    if !version_matches || !upgrade_matches {
        return response(
            StatusCode::BAD_REQUEST,
            "text/plain; charset=utf-8",
            "Invalid WebSocket upgrade\n",
        );
    }
    let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(&key);
    let upgraded = hyper::upgrade::on(request);
    tokio::spawn(async move {
        match upgraded.await {
            Ok(stream) => {
                let socket =
                    WebSocketStream::from_raw_socket(TokioIo::new(stream), Role::Server, None)
                        .await;
                if let Err(error) = workspace.serve_socket(socket).await {
                    eprintln!("xo-syncd synchronization connection failed: {error:#}");
                }
            }
            Err(error) => eprintln!("xo-syncd WebSocket upgrade failed: {error}"),
        }
    });
    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-accept", accept)
        .body(Full::new(Bytes::new()))
        .expect("static WebSocket response is valid")
}

fn response(
    status: StatusCode,
    content_type: &'static str,
    body: impl Into<Bytes>,
) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .header("x-content-type-options", "nosniff")
        .body(Full::new(body.into()))
        .expect("static HTTP response is valid")
}

#[cfg(test)]
mod tests {
    use automerge::sync::State as SyncState;
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio_tungstenite::tungstenite::Message;
    use xo_core::automerge_store::AutomergeRecordStore;
    use xo_core::central_sync::ControlMessage;
    use xo_core::domain::{Frontmatter, FrontmatterValue};
    use xo_core::{Note, NoteId};

    use super::*;

    async fn request(address: std::net::SocketAddr, request: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn item_get_patch_and_delete_use_immutable_records() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = CentralWorkspace::open(directory.path()).unwrap();
        let note_id = NoteId::new("central");
        let created = workspace
            .create_item(&Note {
                id: note_id.clone(),
                frontmatter: Frontmatter::from([
                    (
                        "id".to_owned(),
                        FrontmatterValue::String(note_id.to_string()),
                    ),
                    (
                        "title".to_owned(),
                        FrontmatterValue::String("Before".to_owned()),
                    ),
                ]),
                body: "original".to_owned(),
                path: "ignored.md".to_owned(),
            })
            .await
            .unwrap();
        assert!(created);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(serve(listener, Arc::clone(&workspace), shutdown_rx));

        let get = request(
            address,
            "GET /api/items/central HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(get.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(get.contains(r#""body":"original""#));

        let patch_body = r#"{"body":"updated"}"#;
        let patch = request(
            address,
            &format!(
                "PATCH /api/items/central HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{patch_body}",
                patch_body.len()
            ),
        )
        .await;
        assert!(patch.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(patch.contains(r#""body":"updated""#));
        assert_eq!(workspace.revision_count(&note_id).await.unwrap(), 2);

        let mismatch = r#"{"frontmatter":{"id":"another"}}"#;
        let mismatch_response = request(
            address,
            &format!(
                "PATCH /api/items/central HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{mismatch}",
                mismatch.len()
            ),
        )
        .await;
        assert!(mismatch_response.starts_with("HTTP/1.1 409 Conflict\r\n"));

        let deleted = request(
            address,
            "DELETE /api/items/central HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(deleted.starts_with("HTTP/1.1 204 No Content\r\n"));
        let missing = request(
            address,
            "GET /api/items/central HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(missing.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert_eq!(workspace.revision_count(&note_id).await.unwrap(), 3);

        let _ = shutdown_tx.send(());
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn health_is_exact_and_sync_performs_versioned_hello() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = CentralWorkspace::open(directory.path()).unwrap();
        let workspace_id = workspace.workspace_id().to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(serve(listener, Arc::clone(&workspace), shutdown_rx));

        let mut health = tokio::net::TcpStream::connect(address).await.unwrap();
        health
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut health_response = String::new();
        health.read_to_string(&mut health_response).await.unwrap();
        assert!(health_response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(health_response.ends_with("\r\n\r\nok\n"));

        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/api/sync"))
            .await
            .unwrap();
        socket
            .send(Message::Text(
                ControlMessage::client_hello("test-client")
                    .encode()
                    .unwrap()
                    .into(),
            ))
            .await
            .unwrap();
        let Message::Text(hello) = socket.next().await.unwrap().unwrap() else {
            panic!("server hello was not a text frame");
        };
        assert_eq!(
            ControlMessage::decode(&hello).unwrap(),
            ControlMessage::server_hello(workspace_id.clone(), ["test-client".to_owned()])
        );

        let mut replica = AutomergeRecordStore::create(workspace_id, b"test-client-actor").unwrap();
        let mut sync_state = SyncState::new();
        for _ in 0..4 {
            if let Ok(Some(Ok(Message::Binary(message)))) =
                tokio::time::timeout(std::time::Duration::from_millis(100), socket.next()).await
            {
                replica
                    .receive_sync_message(&mut sync_state, &message)
                    .unwrap();
            }
            if let Some(message) = replica.generate_sync_message(&mut sync_state) {
                socket.send(Message::Binary(message.into())).await.unwrap();
            }
        }
        replica
            .put("test/central-sync", b"durable".to_vec())
            .unwrap();
        let message = replica
            .generate_sync_message(&mut sync_state)
            .expect("local write generates a sync message");
        socket.send(Message::Binary(message.into())).await.unwrap();
        let mut synchronized = false;
        for _ in 0..50 {
            if workspace
                .record("test/central-sync")
                .await
                .unwrap()
                .as_deref()
                == Some(b"durable")
            {
                synchronized = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(synchronized, "server did not durably apply client change");
        socket.close(None).await.unwrap();
        let _ = shutdown_tx.send(());
        task.await.unwrap().unwrap();
    }
}
