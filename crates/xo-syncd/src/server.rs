use std::convert::Infallible;
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Role;

use crate::central::CentralWorkspace;

type Body = Full<Bytes>;

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
                        async move { Ok::<_, Infallible>(handle(request, workspace)) }
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

fn handle(mut request: Request<Incoming>, workspace: Arc<CentralWorkspace>) -> Response<Body> {
    match (request.method(), request.uri().path()) {
        (&Method::GET, "/healthz") => response(StatusCode::OK, "text/plain; charset=utf-8", "ok\n"),
        (&Method::GET, "/api/sync") => websocket_upgrade(&mut request, workspace),
        _ if request.uri().path().starts_with("/api/") => response(
            StatusCode::NOT_FOUND,
            "application/json",
            r#"{"error":"not found"}"#,
        ),
        _ => response(
            StatusCode::SERVICE_UNAVAILABLE,
            "text/plain; charset=utf-8",
            "PWA assets are not embedded in this migration build\n",
        ),
    }
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

    use super::*;

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
