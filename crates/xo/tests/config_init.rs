use std::process::Command;

use anyhow::{Context, Result, ensure};
use xo::config::XoConfig;
use xo::session::WorkspaceSession;

#[tokio::test]
async fn config_init_output_starts_a_fresh_xo_workspace() -> Result<()> {
    let output = Command::new(env!("CARGO_BIN_EXE_xo"))
        .arg("config-init")
        .output()
        .context("run xo config-init")?;
    ensure!(
        output.status.success(),
        "xo config-init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let directory = tempfile::tempdir()?;
    let config_path = directory.path().join("config.scm");
    std::fs::write(&config_path, output.stdout)?;
    let config = XoConfig::load(&config_path, directory.path())?;

    let mut session =
        WorkspaceSession::open(&config.state_dir, None, None, config.projection).await?;
    let behavior = session.behavior().await?;
    ensure!(behavior.views.iter().any(|view| view.id == "notes"));
    ensure!(behavior.views.iter().any(|view| view.id == "all"));
    ensure!(!directory.path().join("notes/xo.scm").exists());
    ensure!(
        session
            .workspace_config_source()
            .await?
            .starts_with("(workspace-config")
    );
    session.shutdown().await
}
