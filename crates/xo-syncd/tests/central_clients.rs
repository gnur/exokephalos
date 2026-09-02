use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use xo::central_client::{CentralClient, CentralClientStatus};
use xo::session::WorkspaceSession;
use xo_core::central_replica::CentralReplica;
use xo_core::domain::{Frontmatter, FrontmatterValue};
use xo_core::record_workspace::RecordWorkspace as _;
use xo_core::{ActorId, Note, NoteId};

struct Server(Child);

impl Server {
    fn start(state: &std::path::Path, address: std::net::SocketAddr) -> Result<Self> {
        let child = Command::new(env!("CARGO_BIN_EXE_xo-syncd"))
            .args([
                "--state-dir",
                state.to_str().context("state path")?,
                "--bind",
            ])
            .arg(address.to_string())
            .arg("--unsafe-disable-auth")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self(child))
    }

    fn stop(mut self) -> Result<()> {
        self.0.kill()?;
        self.0.wait()?;
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn two_native_replicas_converge_through_the_central_server() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let state = directory.path().join("server");
    let _server = Server::start(&state, address)?;
    let workspace_file = state.join("workspace-id");
    let workspace_id = wait_for_workspace_id(&workspace_file).await?;
    let first = CentralReplica::open(
        &directory.path().join("first"),
        &workspace_id,
        ActorId::new("first"),
        b"first-automerge-actor",
    )?;
    let second = CentralReplica::open(
        &directory.path().join("second"),
        &workspace_id,
        ActorId::new("second"),
        b"second-automerge-actor",
    )?;
    let first_client = CentralClient::start(
        &format!("http://{address}"),
        "first".to_owned(),
        Arc::clone(&first),
        None,
    )?;
    let second_client = CentralClient::start(
        &format!("http://{address}"),
        "second".to_owned(),
        Arc::clone(&second),
        None,
    )?;
    wait_connected(&first_client).await?;
    wait_connected(&second_client).await?;

    first
        .put_record("test/native-convergence", b"central".to_vec())
        .await?;
    for _ in 0..200 {
        if second
            .get_record("test/native-convergence")
            .await?
            .as_deref()
            == Some(b"central")
        {
            let mut first_tui = WorkspaceSession::open_central(
                &directory.path().join("first-tui"),
                &format!("http://{address}"),
                directory.path().join("first-projection"),
                xo_core::ClientId::parse("first-tui")?,
            )
            .await?;
            first_tui.behavior().await?;
            let second_tui = WorkspaceSession::open_central(
                &directory.path().join("second-tui"),
                &format!("http://{address}"),
                directory.path().join("second-projection"),
                xo_core::ClientId::parse("second-tui")?,
            )
            .await?;
            let note_id = NoteId::new("central");
            first_tui
                .save(&Note {
                    id: note_id.clone(),
                    path: "central.md".to_owned(),
                    frontmatter: Frontmatter::from([
                        (
                            "id".to_owned(),
                            FrontmatterValue::String(note_id.to_string()),
                        ),
                        (
                            "title".to_owned(),
                            FrontmatterValue::String("Central TUI".to_owned()),
                        ),
                        (
                            "type".to_owned(),
                            FrontmatterValue::String("note".to_owned()),
                        ),
                    ]),
                    body: "through the WebSocket server".to_owned(),
                })
                .await?;
            let mut tui_converged = false;
            for _ in 0..200 {
                if second_tui
                    .snapshot()
                    .await?
                    .notes
                    .iter()
                    .any(|note| note.id == note_id && note.body == "through the WebSocket server")
                {
                    tui_converged = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(
                tui_converged,
                "TUI sessions did not converge through /api/sync"
            );

            let http = reqwest::Client::new();
            let patched = http
                .patch(format!("http://{address}/api/items/{note_id}"))
                .header("content-type", "application/json")
                .body(r#"{"body":"updated through HTTP"}"#)
                .send()
                .await?;
            assert_eq!(patched.status(), reqwest::StatusCode::OK);
            wait_for_note(&second_tui, &note_id, Some("updated through HTTP")).await?;

            second_tui.shutdown().await?;
            let patched_offline = http
                .patch(format!("http://{address}/api/items/{note_id}"))
                .header("content-type", "application/json; charset=utf-8")
                .body(r#"{"body":"changed while client was offline"}"#)
                .send()
                .await?;
            assert_eq!(patched_offline.status(), reqwest::StatusCode::OK);
            let second_tui = WorkspaceSession::open_central(
                &directory.path().join("second-tui"),
                &format!("http://{address}"),
                directory.path().join("second-projection"),
                xo_core::ClientId::parse("second-tui")?,
            )
            .await?;
            wait_for_note(
                &second_tui,
                &note_id,
                Some("changed while client was offline"),
            )
            .await?;

            let deleted = http
                .delete(format!("http://{address}/api/items/{note_id}"))
                .send()
                .await?;
            assert_eq!(deleted.status(), reqwest::StatusCode::NO_CONTENT);
            wait_for_note(&second_tui, &note_id, None).await?;

            first_tui.shutdown().await?;
            second_tui.shutdown().await?;
            first_client.shutdown().await?;
            second_client.shutdown().await?;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    bail!("native replicas did not converge through /api/sync")
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn three_clients_retain_offline_conflicts_through_server_restart() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let state = directory.path().join("conflict-server");
    let server = Server::start(&state, address)?;
    let workspace_id = wait_for_workspace_id(&state.join("workspace-id")).await?;
    wait_for_server(address).await?;
    let server_url = format!("http://{address}");
    let note_id = NoteId::new("offline");

    let mut first = open_session(directory.path(), "first-conflict", &server_url).await?;
    let second = open_session(directory.path(), "second-conflict", &server_url).await?;
    let third = open_session(directory.path(), "third-conflict", &server_url).await?;
    first.save(&test_note(&note_id, "shared base")).await?;
    wait_for_note(&second, &note_id, Some("shared base")).await?;
    wait_for_note(&third, &note_id, Some("shared base")).await?;
    first.shutdown().await?;
    second.shutdown().await?;
    third.shutdown().await?;
    server.stop()?;

    let mut second = WorkspaceSession::open(
        &directory.path().join("second-conflict"),
        Some(&workspace_id),
        directory.path().join("second-conflict-projection"),
    )?;
    second.save(&test_note(&note_id, "offline second")).await?;
    second.shutdown().await?;
    let mut third = WorkspaceSession::open(
        &directory.path().join("third-conflict"),
        Some(&workspace_id),
        directory.path().join("third-conflict-projection"),
    )?;
    third.save(&test_note(&note_id, "offline third")).await?;
    third.shutdown().await?;

    let restarted = Server::start(&state, address)?;
    wait_for_server(address).await?;
    let first = open_session(directory.path(), "first-conflict", &server_url).await?;
    let second = open_session(directory.path(), "second-conflict", &server_url).await?;
    let third = open_session(directory.path(), "third-conflict", &server_url).await?;
    let first_result = wait_for_conflict(&first, &note_id).await?;
    let second_result = wait_for_conflict(&second, &note_id).await?;
    let third_result = wait_for_conflict(&third, &note_id).await?;
    assert_eq!(first_result, second_result);
    assert_eq!(second_result, third_result);
    first.shutdown().await?;
    second.shutdown().await?;
    third.shutdown().await?;
    restarted.stop()?;

    let _restarted_again = Server::start(&state, address)?;
    wait_for_server(address).await?;
    let recovered = open_session(directory.path(), "recovered-conflict", &server_url).await?;
    assert_eq!(wait_for_conflict(&recovered, &note_id).await?, first_result);
    recovered.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn acknowledged_server_data_survives_restart() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let state = directory.path().join("server-restart");
    let server = Server::start(&state, address)?;
    let workspace_id = wait_for_workspace_id(&state.join("workspace-id")).await?;
    let writer = open_replica(directory.path(), "writer", &workspace_id)?;
    let writer_client = CentralClient::start(
        &format!("http://{address}"),
        "writer".to_owned(),
        Arc::clone(&writer),
        None,
    )?;
    wait_connected(&writer_client).await?;
    writer
        .put_record("test/restart", b"persisted".to_vec())
        .await?;

    let observer = open_replica(directory.path(), "observer", &workspace_id)?;
    let observer_client = CentralClient::start(
        &format!("http://{address}"),
        "observer".to_owned(),
        Arc::clone(&observer),
        None,
    )?;
    wait_for_record(&observer, "test/restart", b"persisted").await?;
    writer_client.shutdown().await?;
    observer_client.shutdown().await?;
    server.stop()?;

    let _restarted = Server::start(&state, address)?;
    let recovered = open_replica(directory.path(), "recovered", &workspace_id)?;
    let recovered_client = CentralClient::start(
        &format!("http://{address}"),
        "recovered".to_owned(),
        Arc::clone(&recovered),
        None,
    )?;
    wait_connected(&recovered_client).await?;
    wait_for_record(&recovered, "test/restart", b"persisted").await?;
    recovered_client.shutdown().await?;
    Ok(())
}

async fn open_session(
    root: &std::path::Path,
    name: &str,
    server: &str,
) -> Result<WorkspaceSession> {
    WorkspaceSession::open_central(
        &root.join(name),
        server,
        root.join(format!("{name}-projection")),
        xo_core::ClientId::parse(name)?,
    )
    .await
}

fn test_note(note_id: &NoteId, body: &str) -> Note {
    Note {
        id: note_id.clone(),
        path: "offline-conflict.md".to_owned(),
        frontmatter: Frontmatter::from([
            (
                "id".to_owned(),
                FrontmatterValue::String(note_id.to_string()),
            ),
            (
                "title".to_owned(),
                FrontmatterValue::String("Offline conflict".to_owned()),
            ),
            (
                "type".to_owned(),
                FrontmatterValue::String("note".to_owned()),
            ),
        ]),
        body: body.to_owned(),
    }
}

async fn wait_for_conflict(
    session: &WorkspaceSession,
    note_id: &NoteId,
) -> Result<xo_core::ResolvedNote> {
    for _ in 0..400 {
        if let Some(resolved) = session
            .snapshot()
            .await?
            .resolved
            .into_iter()
            .find(|resolved| {
                resolved.conflict.as_ref().is_some_and(|conflict| {
                    &conflict.note_id == note_id && conflict.concurrent_revisions.len() == 1
                })
            })
        {
            return Ok(resolved);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    bail!("clients did not converge on the retained offline conflict")
}

fn open_replica(
    root: &std::path::Path,
    name: &str,
    workspace_id: &str,
) -> Result<Arc<CentralReplica>> {
    CentralReplica::open(
        &root.join(name),
        workspace_id,
        ActorId::new(name),
        format!("{name}-automerge-actor").as_bytes(),
    )
}

async fn wait_for_record(replica: &CentralReplica, key: &str, expected: &[u8]) -> Result<()> {
    for _ in 0..200 {
        if replica.get_record(key).await?.as_deref() == Some(expected) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    bail!("replica did not receive {key}")
}

async fn wait_for_note(
    session: &WorkspaceSession,
    note_id: &NoteId,
    expected_body: Option<&str>,
) -> Result<()> {
    for _ in 0..200 {
        let body = session
            .snapshot()
            .await?
            .notes
            .into_iter()
            .find(|note| &note.id == note_id)
            .map(|note| note.body);
        if body.as_deref() == expected_body {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    bail!("API item change did not synchronize to the native replica")
}

async fn wait_for_server(address: std::net::SocketAddr) -> Result<()> {
    for _ in 0..200 {
        if reqwest::get(format!("http://{address}/healthz"))
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    bail!("xo-syncd did not become ready")
}

async fn wait_for_workspace_id(path: &std::path::Path) -> Result<String> {
    for _ in 0..200 {
        if let Ok(value) = std::fs::read_to_string(path) {
            return Ok(value.trim().to_owned());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    bail!("xo-syncd did not create its workspace")
}

async fn wait_connected(client: &CentralClient) -> Result<()> {
    for _ in 0..200 {
        if client.status() == CentralClientStatus::Connected {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    bail!("central client did not connect: {:?}", client.status())
}
