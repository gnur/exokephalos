use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use xo_core::iroh_node::{IrohNode, writable_ticket_workspace_id};
use xo_core::records::WorkspaceRecords;
use xo_core::{ActorId, CURRENT_SCHEMA, DeviceRecord};

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
    node: Option<Arc<IrohNode>>,
    endpoint_id: String,
    author_id: String,
    state_dir: PathBuf,
    workspace_ids: RwLock<Vec<String>>,
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
    pub fn new(node: Arc<IrohNode>, workspace_ids: Vec<String>, token: String) -> Self {
        Self {
            inner: Arc::new(OperatorStateInner {
                endpoint_id: node.endpoint_id().to_string(),
                author_id: node.author_id().to_string(),
                state_dir: node.state_dir().to_path_buf(),
                node: Some(node),
                workspace_ids: RwLock::new(workspace_ids),
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
                            let state = state.clone();
                            async move {
                                Ok::<_, Infallible>(handle(request, &state).await)
                            }
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

async fn handle(request: Request<Incoming>, state: &OperatorState) -> Response<Body> {
    state.inner.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let authorization = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if request.method() == Method::POST && request.uri().path() == "/setup" {
        return handle_setup(request, authorization.as_deref(), state).await;
    }
    route(
        request.method(),
        request.uri().path(),
        authorization.as_deref(),
        state,
    )
}

fn route(
    method: &Method,
    path: &str,
    authorization: Option<&str>,
    state: &OperatorState,
) -> Response<Body> {
    if method == Method::GET && path == "/healthz" {
        return json_response(StatusCode::OK, &json!({ "status": "ok" }));
    }
    if method == Method::GET && path == "/readyz" {
        return json_response(StatusCode::OK, &json!({ "status": "ready" }));
    }
    if method == Method::GET && (path == "/" || path == "/setup") {
        return setup_page(StatusCode::OK, None, None);
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
            let workspace_count = state.inner.workspace_ids.read().map_or(0, |ids| ids.len());
            let status = DaemonStatus {
                status: "ok",
                endpoint_id: &state.inner.endpoint_id,
                author_id: &state.inner.author_id,
                state_dir: &state.inner.state_dir,
                workspaces: workspace_count,
                state_bytes: directory_size(&state.inner.state_dir).unwrap_or_else(|_| {
                    state.inner.metrics.errors.fetch_add(1, Ordering::Relaxed);
                    0
                }),
                uptime_seconds: state.inner.started.elapsed().as_secs(),
            };
            json_response(StatusCode::OK, &status)
        }
        (&Method::GET, "/v1/workspaces") => {
            let workspaces = state
                .inner
                .workspace_ids
                .read()
                .map_or_else(|_| Vec::new(), |ids| ids.clone());
            json_response(StatusCode::OK, &json!({ "workspaces": workspaces }))
        }
        (&Method::GET, "/metrics") => metrics_response(state),
        _ => json_response(StatusCode::NOT_FOUND, &json!({ "error": "not found" })),
    }
}

#[allow(clippy::too_many_lines)]
async fn handle_setup(
    request: Request<Incoming>,
    authorization: Option<&str>,
    state: &OperatorState,
) -> Response<Body> {
    const MAX_FORM_BYTES: usize = 256 * 1024;
    let content_length = request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if content_length.is_some_and(|length| length > MAX_FORM_BYTES) {
        return setup_page(
            StatusCode::PAYLOAD_TOO_LARGE,
            Some("The submitted form is too large."),
            None,
        );
    }
    let body = if let Ok(body) = request.into_body().collect().await {
        body.to_bytes()
    } else {
        state.inner.metrics.errors.fetch_add(1, Ordering::Relaxed);
        return setup_page(
            StatusCode::BAD_REQUEST,
            Some("The submitted form could not be read."),
            None,
        );
    };
    if body.len() > MAX_FORM_BYTES {
        return setup_page(
            StatusCode::PAYLOAD_TOO_LARGE,
            Some("The submitted form is too large."),
            None,
        );
    }
    let fields = match parse_form(&body) {
        Ok(fields) => fields,
        Err(error) => {
            return setup_page(StatusCode::BAD_REQUEST, Some(&error), None);
        }
    };
    let form_token = fields.get("operator_token").map(String::as_str);
    if !authorized(authorization, &state.inner.token)
        && !form_token
            .is_some_and(|token| token.as_bytes().ct_eq(state.inner.token.as_slice()).into())
    {
        state
            .inner
            .metrics
            .unauthorized
            .fetch_add(1, Ordering::Relaxed);
        return setup_page(
            StatusCode::UNAUTHORIZED,
            Some("The operator token is incorrect."),
            None,
        );
    }
    let workspace_id = fields.get("workspace_id").map_or("", String::as_str).trim();
    let ticket = fields.get("ticket").map_or("", String::as_str).trim();
    if workspace_id.is_empty() || ticket.is_empty() {
        return setup_page(
            StatusCode::BAD_REQUEST,
            Some("Workspace ID and writable ticket are required."),
            None,
        );
    }
    let ticket_workspace = match writable_ticket_workspace_id(ticket) {
        Ok(workspace) => workspace,
        Err(error) => {
            return setup_page(StatusCode::BAD_REQUEST, Some(&error.to_string()), None);
        }
    };
    if ticket_workspace != workspace_id {
        return setup_page(
            StatusCode::BAD_REQUEST,
            Some("The ticket belongs to a different workspace ID."),
            None,
        );
    }
    let Some(node) = &state.inner.node else {
        state.inner.metrics.errors.fetch_add(1, Ordering::Relaxed);
        return setup_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            Some("Workspace setup is unavailable."),
            None,
        );
    };
    log_event(
        "info",
        "workspace_setup_started",
        &json!({ "workspace_id": workspace_id }),
    );
    let workspace = match node.import_writable_workspace(ticket).await {
        Ok(workspace) => workspace,
        Err(error) => {
            state.inner.metrics.errors.fetch_add(1, Ordering::Relaxed);
            return setup_page(
                StatusCode::BAD_REQUEST,
                Some(&format!("Could not import the workspace: {error}")),
                None,
            );
        }
    };
    if let Err(error) = workspace.sync_and_wait(ticket).await {
        state.inner.metrics.errors.fetch_add(1, Ordering::Relaxed);
        log_event(
            "error",
            "workspace_sync_failed",
            &json!({ "workspace_id": workspace_id, "error": error.to_string() }),
        );
        return setup_page(
            StatusCode::BAD_GATEWAY,
            Some(&format!(
                "Workspace imported, but initial synchronization did not complete: {error}"
            )),
            None,
        );
    }
    if let Err(error) = WorkspaceRecords::new(&workspace)
        .put_device(&DeviceRecord {
            schema: CURRENT_SCHEMA,
            endpoint_id: node.endpoint_id().to_string(),
            author_id: ActorId::new(workspace.author_id().to_string()),
            label: "xo-syncd".to_owned(),
            capabilities: std::collections::BTreeSet::from([
                "write".to_owned(),
                "daemon".to_owned(),
            ]),
            last_seen_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|duration| duration.as_millis().try_into().ok()),
            retired_at: None,
        })
        .await
    {
        state.inner.metrics.errors.fetch_add(1, Ordering::Relaxed);
        log_event(
            "error",
            "workspace_device_registration_failed",
            &json!({ "workspace_id": workspace_id, "error": error.to_string() }),
        );
        return setup_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            Some(&format!(
                "Synchronization succeeded, but daemon registration failed: {error}"
            )),
            None,
        );
    }
    log_event(
        "info",
        "workspace_sync_established",
        &json!({
            "workspace_id": workspace_id,
            "endpoint_id": node.endpoint_id().to_string(),
        }),
    );
    crate::spawn_workspace_logging(workspace.clone(), workspace_id.to_owned());
    let server_ticket = match workspace.share(true).await {
        Ok(ticket) => ticket,
        Err(error) => {
            state.inner.metrics.errors.fetch_add(1, Ordering::Relaxed);
            return setup_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                Some(&format!(
                    "Could not create the server return ticket: {error}"
                )),
                None,
            );
        }
    };
    if let Ok(mut ids) = state.inner.workspace_ids.write()
        && !ids.iter().any(|id| id == workspace_id)
    {
        ids.push(workspace_id.to_owned());
        ids.sort();
    }
    log_event(
        "info",
        "workspace_configured",
        &json!({ "workspace_id": workspace_id }),
    );
    setup_page(StatusCode::OK, None, Some((workspace_id, &server_ticket)))
}

fn setup_page(
    status: StatusCode,
    error: Option<&str>,
    connected: Option<(&str, &str)>,
) -> Response<Body> {
    let notice = error.map_or_else(String::new, |error| {
        format!(
            "<p class=\"notice error\" role=\"alert\">{}</p>",
            html_escape(error)
        )
    });
    let result = connected.map_or_else(String::new, |(workspace_id, ticket)| {
        format!(
            "<section class=\"result\"><h2>Server connected</h2>\
             <p>Workspace <code>{}</code> is now stored and synchronizing.</p>\
             <p>Paste this server ticket into the originating xo client to complete the bidirectional connection:</p>\
             <textarea readonly rows=\"7\">{}</textarea></section>",
            html_escape(workspace_id),
            html_escape(ticket)
        )
    });
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>Configure xo-syncd</title><style>\
         :root{{color-scheme:light dark;font-family:system-ui,sans-serif}}\
         body{{margin:0;background:#10151d;color:#edf3fa}}main{{max-width:44rem;margin:4rem auto;padding:2rem}}\
         .card{{background:#18212d;border:1px solid #344256;border-radius:14px;padding:2rem;box-shadow:0 16px 50px #0005}}\
         h1{{margin-top:0}}label{{display:block;font-weight:650;margin-top:1rem}}\
         input,textarea{{box-sizing:border-box;width:100%;margin-top:.4rem;padding:.75rem;border:1px solid #52647b;border-radius:8px;background:#0e141c;color:inherit}}\
         button{{margin-top:1.25rem;padding:.8rem 1.1rem;border:0;border-radius:8px;background:#5aa9ff;color:#07111d;font-weight:750;cursor:pointer}}\
         .hint{{color:#aebdce}}.notice{{padding:.8rem;border-radius:8px}}.error{{background:#652a32}}\
         .result{{margin-top:1.5rem;padding-top:1rem;border-top:1px solid #344256}}code{{overflow-wrap:anywhere}}\
         </style></head><body><main><div class=\"card\"><h1>Configure xo-syncd</h1>\
         <p class=\"hint\">Attach this server to an existing xo workspace. The ticket is used once and is never logged.</p>\
         {notice}<form method=\"post\" action=\"/setup\" autocomplete=\"off\">\
         <label for=\"operator_token\">Operator token</label>\
         <input id=\"operator_token\" name=\"operator_token\" type=\"password\" required>\
         <label for=\"workspace_id\">Workspace ID</label>\
         <input id=\"workspace_id\" name=\"workspace_id\" required spellcheck=\"false\">\
         <label for=\"ticket\">Writable workspace ticket</label>\
         <textarea id=\"ticket\" name=\"ticket\" rows=\"7\" required spellcheck=\"false\"></textarea>\
         <button type=\"submit\">Connect workspace</button></form>{result}</div></main></body></html>"
    );
    html_response(status, body)
}

fn parse_form(body: &[u8]) -> Result<BTreeMap<String, String>, String> {
    let body = std::str::from_utf8(body).map_err(|_| "Form data is not UTF-8.".to_owned())?;
    body.split('&')
        .filter(|field| !field.is_empty())
        .map(|field| {
            let (key, value) = field.split_once('=').unwrap_or((field, ""));
            Ok((decode_form_component(key)?, decode_form_component(value)?))
        })
        .collect()
}

fn decode_form_component(value: &str) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(value.len());
    let value = value.as_bytes();
    let mut index = 0;
    while index < value.len() {
        match value[index] {
            b'+' => bytes.push(b' '),
            b'%' if index + 2 < value.len() => {
                let high = hex(value[index + 1])?;
                let low = hex(value[index + 2])?;
                bytes.push(high * 16 + low);
                index += 2;
            }
            b'%' => return Err("Form data contains an incomplete escape.".to_owned()),
            byte => bytes.push(byte),
        }
        index += 1;
    }
    String::from_utf8(bytes).map_err(|_| "Form data contains invalid UTF-8.".to_owned())
}

fn hex(value: u8) -> Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err("Form data contains an invalid escape.".to_owned()),
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn authorized(header: Option<&str>, expected: &[u8]) -> bool {
    let Some(provided) = header.and_then(|value| value.strip_prefix("Bearer ")) else {
        return false;
    };
    provided.as_bytes().ct_eq(expected).into()
}

fn html_response(status: StatusCode, body: String) -> Response<Body> {
    let mut response = response(status, "text/html; charset=utf-8", body);
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'",
        ),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
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
    let workspace_count = state.inner.workspace_ids.read().map_or(0, |ids| ids.len());
    let body = format!(
        "# TYPE xo_syncd_up gauge\nxo_syncd_up 1\n\
         # TYPE xo_syncd_uptime_seconds counter\nxo_syncd_uptime_seconds {}\n\
         # TYPE xo_syncd_workspaces gauge\nxo_syncd_workspaces {}\n\
         # TYPE xo_syncd_operator_requests_total counter\nxo_syncd_operator_requests_total {}\n\
         # TYPE xo_syncd_operator_unauthorized_total counter\nxo_syncd_operator_unauthorized_total {}\n\
         # TYPE xo_syncd_operator_errors_total counter\nxo_syncd_operator_errors_total {}\n",
        state.inner.started.elapsed().as_secs(),
        workspace_count,
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
        OperatorState {
            inner: Arc::new(OperatorStateInner {
                node: None,
                endpoint_id: "endpoint".to_owned(),
                author_id: "author".to_owned(),
                state_dir: path.to_path_buf(),
                workspace_ids: RwLock::new(vec!["workspace".to_owned()]),
                token: b"secret-token".to_vec(),
                started: Instant::now(),
                metrics: Metrics {
                    requests: AtomicU64::new(0),
                    unauthorized: AtomicU64::new(0),
                    errors: AtomicU64::new(0),
                },
            }),
        }
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
        let page = route(&Method::GET, "/setup", None, &state);
        assert_eq!(page.status(), StatusCode::OK);
        let body = page.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("Connect workspace"));
    }

    #[test]
    fn setup_form_decodes_fields_and_escapes_html() {
        let fields = parse_form(b"workspace_id=abc%2F123&ticket=hello+world").expect("valid form");
        assert_eq!(fields["workspace_id"], "abc/123");
        assert_eq!(fields["ticket"], "hello world");
        assert_eq!(
            html_escape("<ticket value=\"secret\">"),
            "&lt;ticket value=&quot;secret&quot;&gt;"
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

    #[tokio::test]
    async fn setup_page_imports_matching_writable_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let source = Arc::new(
            IrohNode::persistent(directory.path().join("source"))
                .await
                .unwrap(),
        );
        let workspace = source.create_workspace().await.unwrap();
        let workspace_id = workspace.id().to_string();
        let ticket = workspace.share(true).await.unwrap();
        let daemon = Arc::new(
            IrohNode::persistent(directory.path().join("daemon"))
                .await
                .unwrap(),
        );
        let token = "a".repeat(32);
        let state = OperatorState::new(Arc::clone(&daemon), Vec::new(), token.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve(listener, state, shutdown_rx));

        let mismatch = http_post_form(
            address,
            &format!(
                "operator_token={}&workspace_id=wrong&ticket={}",
                form_encode(&token),
                form_encode(&ticket)
            ),
        )
        .await;
        assert!(mismatch.starts_with("HTTP/1.1 400"));
        assert!(daemon.workspace_ids().await.unwrap().is_empty());

        let connected = http_post_form(
            address,
            &format!(
                "operator_token={}&workspace_id={}&ticket={}",
                form_encode(&token),
                form_encode(&workspace_id),
                form_encode(&ticket)
            ),
        )
        .await;
        assert!(connected.starts_with("HTTP/1.1 200"));
        assert!(connected.contains("Server connected"));
        assert!(connected.contains(&workspace_id));
        assert!(!connected.contains(&ticket));
        assert_eq!(daemon.workspace_ids().await.unwrap(), vec![workspace_id]);

        shutdown_tx.send(()).unwrap();
        server.await.unwrap().unwrap();
        daemon.shutdown().await.unwrap();
        source.shutdown().await.unwrap();
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

    async fn http_post_form(address: SocketAddr, body: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let request = format!(
            "POST /setup HTTP/1.1\r\nHost: localhost\r\n\
             Content-Type: application/x-www-form-urlencoded\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        String::from_utf8(response).unwrap()
    }

    fn form_encode(value: &str) -> String {
        let mut encoded = String::new();
        for byte in value.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                encoded.push(char::from(byte));
            } else {
                use std::fmt::Write as _;
                write!(&mut encoded, "%{byte:02X}").unwrap();
            }
        }
        encoded
    }
}
