use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

type Body = Full<Bytes>;

#[derive(Debug)]
struct Metrics {
    requests: AtomicU64,
    unauthorized: AtomicU64,
    errors: AtomicU64,
}

#[derive(Clone, Debug)]
pub struct OperatorState {
    inner: Arc<OperatorStateInner>,
}

#[derive(Debug)]
struct OperatorStateInner {
    endpoint_id: String,
    author_id: String,
    state_dir: PathBuf,
    workspace_ids: Vec<String>,
    token: Vec<u8>,
    started: Instant,
    metrics: Metrics,
}

#[derive(Debug, Serialize)]
struct DaemonStatus<'a> {
    status: &'static str,
    endpoint_id: &'a str,
    author_id: &'a str,
    state_dir: &'a Path,
    workspaces: usize,
    state_bytes: u64,
    uptime_seconds: u64,
}

impl OperatorState {
    pub fn new(
        endpoint_id: String,
        author_id: String,
        state_dir: PathBuf,
        workspace_ids: Vec<String>,
        token: String,
    ) -> Self {
        Self {
            inner: Arc::new(OperatorStateInner {
                endpoint_id,
                author_id,
                state_dir,
                workspace_ids,
                token: token.into_bytes(),
                started: Instant::now(),
                metrics: Metrics {
                    requests: AtomicU64::new(0),
                    unauthorized: AtomicU64::new(0),
                    errors: AtomicU64::new(0),
                },
            }),
        }
    }
}

pub async fn serve(
    listener: TcpListener,
    state: OperatorState,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = &mut shutdown => return Ok(()),
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                    let state = state.clone();
                    tokio::spawn(async move {
                        let service = service_fn(move |request| {
                            let response = handle(&request, &state);
                            std::future::ready(Ok::<_, Infallible>(response))
                        });
                    if let Err(error) = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                    {
                        log_event("warn", "operator_connection_failed", &json!({
                            "peer": peer,
                            "error": error.to_string(),
                        }));
                    }
                });
            }
        }
    }
}

fn handle(request: &Request<Incoming>, state: &OperatorState) -> Response<Body> {
    let authorization = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    route(request.method(), request.uri().path(), authorization, state)
}

fn route(
    method: &Method,
    path: &str,
    authorization: Option<&str>,
    state: &OperatorState,
) -> Response<Body> {
    state.inner.metrics.requests.fetch_add(1, Ordering::Relaxed);
    if method == Method::GET && path == "/healthz" {
        return json_response(StatusCode::OK, &json!({ "status": "ok" }));
    }
    if method == Method::GET && path == "/readyz" {
        return json_response(StatusCode::OK, &json!({ "status": "ready" }));
    }
    if !authorized(authorization, &state.inner.token) {
        state
            .inner
            .metrics
            .unauthorized
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::UNAUTHORIZED,
            &json!({ "error": "bearer token required" }),
        );
    }
    match (method, path) {
        (&Method::GET, "/v1/status") => {
            let status = DaemonStatus {
                status: "ok",
                endpoint_id: &state.inner.endpoint_id,
                author_id: &state.inner.author_id,
                state_dir: &state.inner.state_dir,
                workspaces: state.inner.workspace_ids.len(),
                state_bytes: directory_size(&state.inner.state_dir).unwrap_or_else(|_| {
                    state.inner.metrics.errors.fetch_add(1, Ordering::Relaxed);
                    0
                }),
                uptime_seconds: state.inner.started.elapsed().as_secs(),
            };
            json_response(StatusCode::OK, &status)
        }
        (&Method::GET, "/v1/workspaces") => json_response(
            StatusCode::OK,
            &json!({ "workspaces": state.inner.workspace_ids }),
        ),
        (&Method::GET, "/metrics") => metrics_response(state),
        _ => json_response(StatusCode::NOT_FOUND, &json!({ "error": "not found" })),
    }
}

fn authorized(header: Option<&str>, expected: &[u8]) -> bool {
    let Some(provided) = header.and_then(|value| value.strip_prefix("Bearer ")) else {
        return false;
    };
    provided.as_bytes().ct_eq(expected).into()
}

fn json_response(status: StatusCode, value: &impl Serialize) -> Response<Body> {
    match serde_json::to_vec(value) {
        Ok(body) => response(status, "application/json", body),
        Err(_) => response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "application/json",
            br#"{"error":"serialization failed"}"#.to_vec(),
        ),
    }
}

fn metrics_response(state: &OperatorState) -> Response<Body> {
    let body = format!(
        "# TYPE xo_syncd_up gauge\nxo_syncd_up 1\n\
         # TYPE xo_syncd_uptime_seconds counter\nxo_syncd_uptime_seconds {}\n\
         # TYPE xo_syncd_workspaces gauge\nxo_syncd_workspaces {}\n\
         # TYPE xo_syncd_operator_requests_total counter\nxo_syncd_operator_requests_total {}\n\
         # TYPE xo_syncd_operator_unauthorized_total counter\nxo_syncd_operator_unauthorized_total {}\n\
         # TYPE xo_syncd_operator_errors_total counter\nxo_syncd_operator_errors_total {}\n",
        state.inner.started.elapsed().as_secs(),
        state.inner.workspace_ids.len(),
        state.inner.metrics.requests.load(Ordering::Relaxed),
        state.inner.metrics.unauthorized.load(Ordering::Relaxed),
        state.inner.metrics.errors.load(Ordering::Relaxed),
    );
    response(StatusCode::OK, "text/plain; version=0.0.4", body)
}

fn response(
    status: StatusCode,
    content_type: &'static str,
    body: impl Into<Bytes>,
) -> Response<Body> {
    let mut response = Response::new(Full::new(body.into()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        hyper::header::HeaderValue::from_static(content_type),
    );
    response
}

fn directory_size(path: &Path) -> std::io::Result<u64> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in std::fs::read_dir(path)? {
        total = total.saturating_add(directory_size(&entry?.path())?);
    }
    Ok(total)
}

pub fn log_event(level: &str, event: &str, fields: &Value) {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    println!(
        "{}",
        json!({
            "timestamp_ms": timestamp_ms,
            "level": level,
            "event": event,
            "fields": fields,
        })
    );
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use http_body_util::BodyExt as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    fn state(path: &Path) -> OperatorState {
        OperatorState::new(
            "endpoint".to_owned(),
            "author".to_owned(),
            path.to_path_buf(),
            vec!["workspace".to_owned()],
            "secret-token".to_owned(),
        )
    }

    #[tokio::test]
    async fn health_is_public_but_operator_routes_require_a_token() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path());
        assert_eq!(
            route(&Method::GET, "/healthz", None, &state).status(),
            StatusCode::OK
        );
        assert_eq!(
            route(&Method::GET, "/v1/status", None, &state).status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            route(&Method::GET, "/v1/status", Some("Bearer wrong"), &state).status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            route(
                &Method::GET,
                "/v1/status",
                Some("Bearer secret-token"),
                &state
            )
            .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn metrics_are_authenticated_and_prometheus_formatted() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path());
        let response = route(
            &Method::GET,
            "/metrics",
            Some("Bearer secret-token"),
            &state,
        );
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("xo_syncd_up 1"));
        assert!(body.contains("xo_syncd_workspaces 1"));
    }

    #[tokio::test]
    async fn live_listener_serves_authenticated_status() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve(listener, state, shutdown_rx));

        let unauthorized = http_get(address, "/v1/status", None).await;
        assert!(unauthorized.starts_with("HTTP/1.1 401"));
        let authorized = http_get(address, "/v1/status", Some("secret-token")).await;
        assert!(authorized.starts_with("HTTP/1.1 200"));
        assert!(authorized.contains("\"endpoint_id\":\"endpoint\""));

        shutdown_tx.send(()).unwrap();
        server.await.unwrap().unwrap();
    }

    async fn http_get(address: SocketAddr, path: &str, token: Option<&str>) -> String {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let authorization = token.map_or_else(String::new, |token| {
            format!("Authorization: Bearer {token}\r\n")
        });
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\n{authorization}Connection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        String::from_utf8(response).unwrap()
    }
}
