mod auth;
mod central;
mod config;
mod pwa;
mod server;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
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
    /// Server configuration file (defaults to ~/.config/xo-syncd/config.scm).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Override the authoritative Automerge workspace directory from config.scm.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Override the PWA, API, health, and WebSocket listen address from config.scm.
    #[arg(long)]
    bind: Option<SocketAddr>,
    /// Pocket ID issuer URL.
    #[arg(long)]
    oidc_issuer: Option<String>,
    /// Pocket ID API resource expected in access-token audiences.
    #[arg(long)]
    oidc_audience: Option<String>,
    /// Public Pocket ID OIDC client ID used by PWA and native PKCE flows.
    #[arg(long)]
    oidc_client_id: Option<String>,
    /// Disable authentication explicitly. Intended only for local development and tests.
    #[arg(long)]
    unsafe_disable_auth: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print a default ~/.config/xo-syncd/config.scm document to stdout.
    ConfigInit,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    if matches!(cli.command, Some(Command::ConfigInit)) {
        let template = config::SyncdConfig {
            state_dir: Some("~/.local/share/xo-syncd".into()),
            bind: Some("127.0.0.1:9464".into()),
            ..config::SyncdConfig::default()
        };
        print!("{}", template.document());
        return Ok(());
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let explicit_config = cli.config.is_some();
    let config_path = cli
        .config
        .clone()
        .or_else(|| home.as_deref().map(config::default_path));
    let file = match config_path {
        Some(path) => config::SyncdConfig::load_optional(&path, explicit_config, home.as_deref())?,
        None => config::SyncdConfig::default(),
    };
    let state_dir = cli
        .state_dir
        .or(file.state_dir)
        .unwrap_or_else(|| PathBuf::from(".xo/syncd"));
    let bind = match cli.bind {
        Some(bind) => bind,
        None => file
            .bind
            .as_deref()
            .unwrap_or("127.0.0.1:9464")
            .parse()
            .context("invalid xo-syncd bind address in configuration")?,
    };
    let oidc_issuer = cli.oidc_issuer.or(file.oidc_issuer);
    let oidc_audience = cli.oidc_audience.or(file.oidc_audience);
    let oidc_client_id = cli.oidc_client_id.or(file.oidc_client_id);
    let auth = if cli.unsafe_disable_auth {
        auth::Authenticator::unsafe_disabled()
    } else {
        let issuer = oidc_issuer.as_deref().context("oidc-issuer is required")?;
        let audience = oidc_audience
            .as_deref()
            .context("oidc-audience is required")?;
        let client_id = oidc_client_id
            .as_deref()
            .context("oidc-client-id is required")?;
        auth::Authenticator::discover(issuer, audience, client_id).await?
    };
    let workspace = CentralWorkspace::open(&state_dir)?;
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind xo-syncd to {bind}"))?;
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
