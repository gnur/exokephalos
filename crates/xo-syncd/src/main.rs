mod central;
mod server;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::Parser;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use central::CentralWorkspace;

#[derive(Debug, Parser)]
#[command(
    name = "xo-syncd",
    version = xo_core::version::VERSION,
    about = "Central xo Automerge synchronization server"
)]
struct Cli {
    /// Directory containing the server's authoritative Automerge workspace.
    #[arg(long, default_value = ".xo/syncd")]
    state_dir: PathBuf,
    /// Address serving the PWA, item API, health probe, and WebSocket sync.
    #[arg(long, default_value = "127.0.0.1:9464")]
    bind: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let workspace = CentralWorkspace::open(&cli.state_dir)?;
    let listener = TcpListener::bind(cli.bind)
        .await
        .with_context(|| format!("bind xo-syncd to {}", cli.bind))?;
    let address = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(server::serve(listener, workspace.clone(), shutdown_rx));
    eprintln!(
        "xo-syncd serving workspace {} on http://{}",
        workspace.workspace_id(),
        address
    );

    tokio::signal::ctrl_c().await?;
    let _ = shutdown_tx.send(());
    server.await.context("join xo-syncd HTTP server")??;
    Ok(())
}
