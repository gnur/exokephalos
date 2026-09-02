mod auth;
mod central;
mod pwa;
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
    /// Pocket ID issuer URL.
    #[arg(long)]
    oidc_issuer: Option<String>,
    /// Pocket ID API resource expected in access-token audiences.
    #[arg(long)]
    oidc_audience: Option<String>,
    /// Public Pocket ID OIDC client ID used by the PWA and TUI device flow.
    #[arg(long)]
    oidc_client_id: Option<String>,
    /// Disable authentication explicitly. Intended only for local development and tests.
    #[arg(long)]
    unsafe_disable_auth: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    let auth = if cli.unsafe_disable_auth {
        auth::Authenticator::unsafe_disabled()
    } else {
        let issuer = cli
            .oidc_issuer
            .as_deref()
            .context("--oidc-issuer is required")?;
        let audience = cli
            .oidc_audience
            .as_deref()
            .context("--oidc-audience is required")?;
        let client_id = cli
            .oidc_client_id
            .as_deref()
            .context("--oidc-client-id is required")?;
        auth::Authenticator::discover(issuer, audience, client_id).await?
    };
    let workspace = CentralWorkspace::open(&cli.state_dir)?;
    let listener = TcpListener::bind(cli.bind)
        .await
        .with_context(|| format!("bind xo-syncd to {}", cli.bind))?;
    let address = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(server::serve(
        listener,
        workspace.clone(),
        std::sync::Arc::new(auth),
        shutdown_rx,
    ));
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
