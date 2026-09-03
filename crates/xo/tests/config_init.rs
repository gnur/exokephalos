use std::process::Command;

use anyhow::{Context, Result, ensure};
use xo::config::XoConfig;
use xo::keymap::{DEFAULT_KEYS, KeyMap};
use xo::session::WorkspaceSession;

#[test]
fn keymap_init_prints_the_default_keymap() -> Result<()> {
    let output = Command::new(env!("CARGO_BIN_EXE_xo"))
        .arg("keymap-init")
        .output()
        .context("run xo keymap-init")?;
    ensure!(
        output.status.success(),
        "xo keymap-init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let source = String::from_utf8(output.stdout)?;
    ensure!(source == DEFAULT_KEYS, "keymap-init output changed");
    KeyMap::from_source(&source)?;
    Ok(())
}

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
    ensure!(config.server == "http://127.0.0.1:9464");

    let mut session = WorkspaceSession::open(&config.state_dir, None, config.projection)?;
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
