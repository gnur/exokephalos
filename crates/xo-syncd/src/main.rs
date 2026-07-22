mod operator;

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use rand::RngCore as _;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use operator::{OperatorState, log_event};

#[derive(Debug, Parser)]
#[command(
    name = "xo-syncd",
    version,
    about = "Durable exokephalos replication peer"
)]
struct Cli {
    /// Directory containing local daemon state.
    #[arg(long, default_value = ".exo/syncd")]
    state_dir: PathBuf,
    /// Address for health, metrics, and authenticated operator endpoints.
    #[arg(long, default_value = "127.0.0.1:9464")]
    operator_bind: SocketAddr,
    /// Bearer-token file; defaults to `STATE_DIR/operator.token` and is created if absent.
    #[arg(long)]
    operator_token_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let node = xo_core::iroh_node::IrohNode::persistent(&cli.state_dir).await?;
    let token_file = cli
        .operator_token_file
        .unwrap_or_else(|| cli.state_dir.join("operator.token"));
    let token = load_or_create_token(&token_file)?;
    let listener = TcpListener::bind(cli.operator_bind)
        .await
        .with_context(|| format!("bind operator server to {}", cli.operator_bind))?;
    let operator_addr = listener.local_addr()?;
    let state = OperatorState::new(
        node.endpoint_id().to_string(),
        node.author_id().to_string(),
        node.state_dir().to_path_buf(),
        node.workspace_ids().await?,
        token,
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let operator_task = tokio::spawn(operator::serve(listener, state, shutdown_rx));
    log_event(
        "info",
        "daemon_started",
        &json!({
            "endpoint_id": node.endpoint_id().to_string(),
            "author_id": node.author_id().to_string(),
            "state_dir": node.state_dir(),
            "operator_addr": operator_addr,
            "operator_token_file": token_file,
        }),
    );

    tokio::signal::ctrl_c().await?;
    log_event("info", "shutdown_requested", &json!({ "signal": "ctrl_c" }));
    let _ = shutdown_tx.send(());
    operator_task.await.context("join operator server")??;
    node.shutdown().await?;
    log_event("info", "daemon_stopped", &json!({}));
    Ok(())
}

fn load_or_create_token(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(token) => validate_token(token.trim()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => create_token(path),
        Err(error) => Err(error).with_context(|| format!("read token file {}", path.display())),
    }
}

fn validate_token(token: &str) -> Result<String> {
    if token.len() < 32 || token.chars().any(char::is_whitespace) {
        bail!("operator token must contain at least 32 non-whitespace characters");
    }
    Ok(token.to_owned())
}

fn create_token(path: &Path) -> Result<String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create token directory {}", parent.display()))?;
    }
    let mut random = [0_u8; 32];
    rand::rng().fill_bytes(&mut random);
    let mut token = String::with_capacity(64);
    for byte in random {
        write!(&mut token, "{byte:02x}").expect("writing to a string cannot fail");
    }
    write_token_file(path, &token)?;
    Ok(token)
}

#[cfg(unix)]
fn write_token_file(path: &Path, token: &str) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create token file {}", path.display()))?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_token_file(path: &Path, token: &str) -> Result<()> {
    use std::io::Write as _;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create token file {}", path.display()))?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_token_is_created_once_and_reused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("operator.token");
        let created = load_or_create_token(&path).unwrap();
        assert_eq!(created.len(), 64);
        assert_eq!(load_or_create_token(&path).unwrap(), created);
    }

    #[test]
    fn short_operator_tokens_are_rejected() {
        assert!(validate_token("too-short").is_err());
    }
}
