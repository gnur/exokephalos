use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

#[test]
fn config_init_prints_the_server_scheme_schema() -> Result<()> {
    let output = Command::new(env!("CARGO_BIN_EXE_xo-syncd"))
        .arg("config-init")
        .output()?;
    assert!(output.status.success());
    let document = String::from_utf8(output.stdout)?;
    assert!(document.contains("(xo-syncd-config"));
    assert!(document.contains("(state-dir \"~/.local/share/xo-syncd\")"));
    assert!(document.contains("(oidc-issuer #f)"));
    Ok(())
}

#[tokio::test]
async fn file_configures_state_and_cli_overrides_bind() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let directory = tempfile::tempdir()?;
    let state = directory.path().join("server-state");
    let configured_address = reserve_address()?;
    let override_address = reserve_address()?;
    let config = directory.path().join("config.scm");
    std::fs::write(
        &config,
        format!(
            "(xo-syncd-config\n  (schema 1)\n  (state-dir {})\n  (bind {})\n  (oidc-issuer #f)\n  (oidc-audience #f)\n  (oidc-client-id #f))\n",
            serde_json::to_string(state.to_str().context("state path is not UTF-8")?)?,
            serde_json::to_string(&configured_address.to_string())?,
        ),
    )?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_xo-syncd"))
        .args([
            "--config",
            config.to_str().context("config path")?,
            "--bind",
        ])
        .arg(override_address.to_string())
        .arg("--unsafe-disable-auth")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let result = async {
        for _ in 0..60 {
            if reqwest::get(format!("http://{override_address}/healthz"))
                .await
                .is_ok_and(|response| response.status().is_success())
                && state.join("workspace-id").is_file()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        bail!("xo-syncd did not start from file configuration with the CLI bind override")
    }
    .await;
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn reserve_address() -> Result<std::net::SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    Ok(address)
}
