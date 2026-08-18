use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use xo::central_client::{CentralClient, CentralClientStatus};
use xo_core::ActorId;
use xo_core::central_replica::CentralReplica;
use xo_core::record_workspace::RecordWorkspace as _;

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn two_native_replicas_converge_through_the_central_server() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let state = directory.path().join("server");
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
    let _server = Server(child);
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
            first_client.shutdown().await?;
            second_client.shutdown().await?;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    bail!("native replicas did not converge through /api/sync")
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
