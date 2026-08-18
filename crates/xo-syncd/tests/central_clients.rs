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
    )?;
    let second_client = CentralClient::start(
        &format!("http://{address}"),
        "second".to_owned(),
        Arc::clone(&second),
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
                xo_core::PeerId::parse("first-tui")?,
            )
            .await?;
            first_tui.behavior().await?;
            let second_tui = WorkspaceSession::open_central(
                &directory.path().join("second-tui"),
                &format!("http://{address}"),
                directory.path().join("second-projection"),
                xo_core::PeerId::parse("second-tui")?,
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
    )?;
    wait_connected(&recovered_client).await?;
    wait_for_record(&recovered, "test/restart", b"persisted").await?;
    recovered_client.shutdown().await?;
    Ok(())
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
