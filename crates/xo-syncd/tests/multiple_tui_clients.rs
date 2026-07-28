use std::collections::BTreeSet;
use std::io::Read as _;
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use xo::session::WorkspaceSession;
use xo_core::domain::{Frontmatter, FrontmatterValue};
use xo_core::iroh_node::IrohNode;
use xo_core::records::WorkspaceRecords;
use xo_core::{Note, NoteId};

struct SyncdProcess {
    child: Child,
    operator_addr: SocketAddr,
}

impl SyncdProcess {
    async fn start(state_dir: &Path) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let operator_addr = listener.local_addr()?;
        drop(listener);
        let child = Command::new(env!("CARGO_BIN_EXE_xo-syncd"))
            .arg("--state-dir")
            .arg(state_dir)
            .arg("--operator-bind")
            .arg(operator_addr.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("start xo-syncd")?;
        let mut process = Self {
            child,
            operator_addr,
        };
        process.wait_until_ready().await?;
        Ok(process)
    }

    async fn wait_until_ready(&mut self) -> Result<()> {
        for _ in 0..200 {
            if let Some(status) = self.child.try_wait().context("poll xo-syncd")? {
                let mut stderr = String::new();
                if let Some(mut stream) = self.child.stderr.take() {
                    stream.read_to_string(&mut stderr)?;
                }
                bail!(
                    "xo-syncd exited before becoming ready: {status}: {}",
                    stderr.trim()
                );
            }
            if tokio::net::TcpStream::connect(self.operator_addr)
                .await
                .is_ok()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        bail!("xo-syncd did not become ready")
    }

    fn stop(mut self) -> Result<()> {
        self.terminate()
    }

    fn terminate(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill().context("stop xo-syncd")?;
        }
        self.child.wait().context("wait for xo-syncd")?;
        Ok(())
    }
}

impl Drop for SyncdProcess {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

#[tokio::test]
#[ignore = "requires public N0 discovery and relay services"]
#[allow(clippy::too_many_lines)]
async fn syncd_restart_converges_two_restarted_tui_clients_with_offline_conflict() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let server_state = directory.path().join("server");
    let client_one_state = directory.path().join("client-one");
    let client_two_state = directory.path().join("client-two");

    let mut client_one = WorkspaceSession::open(
        &client_one_state,
        None,
        None,
        directory.path().join("projection-one"),
    )
    .await?;
    client_one.behavior().await?;
    let workspace_id = client_one.workspace_id();
    let client_ticket = client_one.writable_invitation().await?;

    let server = IrohNode::persistent(&server_state).await?;
    let server_workspace = server.import_writable_workspace(&client_ticket).await?;
    wait_until("initial configuration to reach server", || async {
        WorkspaceRecords::new(&server_workspace)
            .snapshot()
            .await
            .is_ok_and(|snapshot| !snapshot.configs.is_empty())
    })
    .await?;
    let server_ticket = server_workspace.share(true).await?;
    server.shutdown().await?;

    client_one.connect_peer(&server_ticket).await?;
    let daemon = SyncdProcess::start(&server_state).await?;
    let mut client_two = WorkspaceSession::open(
        &client_two_state,
        None,
        Some(&server_ticket),
        directory.path().join("projection-two"),
    )
    .await?;
    wait_until("configuration to reach second TUI client", || async {
        client_two
            .snapshot()
            .await
            .is_ok_and(|snapshot| !snapshot.configs.is_empty())
    })
    .await?;
    client_two.behavior().await?;

    let base = note("aaaaaaa", "Shared note", "base");
    client_one.save(&base).await?;
    wait_until("base note to reach second TUI client", || async {
        has_note(&client_two, &base.id).await
    })
    .await?;

    client_one.shutdown().await?;
    client_two.shutdown().await?;
    daemon.stop()?;

    let mut client_one = WorkspaceSession::open(
        &client_one_state,
        None,
        None,
        directory.path().join("projection-one"),
    )
    .await?;
    let mut client_two = WorkspaceSession::open(
        &client_two_state,
        None,
        None,
        directory.path().join("projection-two"),
    )
    .await?;
    assert_eq!(client_one.workspace_id(), workspace_id);
    assert_eq!(client_two.workspace_id(), workspace_id);

    let mut client_one_edit = find_note(&client_one, &base.id).await?;
    client_one_edit.body = "offline edit from client one".into();
    client_one.save(&client_one_edit).await?;
    let mut client_two_edit = find_note(&client_two, &base.id).await?;
    client_two_edit.body = "offline edit from client two".into();
    client_two.save(&client_two_edit).await?;

    let daemon = SyncdProcess::start(&server_state).await?;
    client_one.connect_peer(&server_ticket).await?;
    client_two.connect_peer(&server_ticket).await?;
    wait_until(
        "offline conflict to converge to both TUI clients",
        || async {
            has_conflict(&client_one, &base.id).await && has_conflict(&client_two, &base.id).await
        },
    )
    .await?;
    assert_history(&client_one, &base.id).await?;
    assert_history(&client_two, &base.id).await?;

    let probe = IrohNode::persistent(directory.path().join("server-probe")).await?;
    let probe_workspace = probe.import_workspace(&server_ticket).await?;
    wait_until("real xo-syncd to serve the converged conflict", || async {
        WorkspaceRecords::new(&probe_workspace)
            .snapshot()
            .await
            .is_ok_and(|snapshot| {
                snapshot.resolved.iter().any(|note| {
                    note.conflict
                        .as_ref()
                        .is_some_and(|conflict| conflict.note_id == base.id)
                })
            })
    })
    .await?;
    probe.shutdown().await?;

    client_one.shutdown().await?;
    client_two.shutdown().await?;
    daemon.stop()?;

    let persisted_server = IrohNode::persistent(&server_state).await?;
    let persisted_workspace = persisted_server
        .open_workspace_str(&workspace_id)
        .await?
        .context("xo-syncd workspace disappeared after restart")?;
    let records = WorkspaceRecords::new(&persisted_workspace);
    let snapshot = retry_until_ok("persisted server snapshot", || async {
        records.snapshot().await.map_err(Into::into)
    })
    .await?;
    assert!(snapshot.resolved.iter().any(|note| {
        note.conflict
            .as_ref()
            .is_some_and(|conflict| conflict.note_id == base.id)
    }));
    let history = retry_until_ok("persisted server revision history", || async {
        records.revision_history(&base.id).await.map_err(Into::into)
    })
    .await?;
    let bodies = history
        .into_iter()
        .map(|(_, revision)| revision.body)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        bodies,
        BTreeSet::from([
            "base".to_owned(),
            "offline edit from client one".to_owned(),
            "offline edit from client two".to_owned(),
        ])
    );
    persisted_server.shutdown().await?;
    Ok(())
}

fn note(id: &str, title: &str, body: &str) -> Note {
    let id = NoteId::new(id);
    let frontmatter = Frontmatter::from([
        (
            "created".into(),
            FrontmatterValue::String("2026-07-26".into()),
        ),
        (
            "id".into(),
            FrontmatterValue::String(id.as_str().to_owned()),
        ),
        ("tags".into(), FrontmatterValue::Sequence(Vec::new())),
        ("title".into(), FrontmatterValue::String(title.into())),
        ("type".into(), FrontmatterValue::String("note".into())),
    ]);
    Note {
        path: xo_core::projection::canonical_note_path(&id, &frontmatter),
        id,
        frontmatter,
        body: body.into(),
    }
}

async fn find_note(session: &WorkspaceSession, id: &NoteId) -> Result<Note> {
    retry_until_ok(&format!("note {id} to be readable"), || async {
        session
            .snapshot()
            .await?
            .notes
            .into_iter()
            .find(|note| &note.id == id)
            .with_context(|| format!("note {id} is missing"))
    })
    .await
}

async fn has_note(session: &WorkspaceSession, id: &NoteId) -> bool {
    session
        .snapshot()
        .await
        .is_ok_and(|snapshot| snapshot.notes.iter().any(|note| &note.id == id))
}

async fn has_conflict(session: &WorkspaceSession, id: &NoteId) -> bool {
    session.snapshot().await.is_ok_and(|snapshot| {
        snapshot.resolved.iter().any(|note| {
            note.conflict
                .as_ref()
                .is_some_and(|conflict| &conflict.note_id == id)
        })
    })
}

async fn assert_history(session: &WorkspaceSession, id: &NoteId) -> Result<()> {
    let history = retry_until_ok(&format!("history for note {id}"), || async {
        session.history(id).await
    })
    .await?;
    let bodies = history
        .into_iter()
        .map(|(_, revision)| revision.body)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        bodies,
        BTreeSet::from([
            "base".to_owned(),
            "offline edit from client one".to_owned(),
            "offline edit from client two".to_owned(),
        ])
    );
    Ok(())
}

async fn retry_until_ok<T, F, Fut>(description: &str, mut operation: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_error = None;
    for _ in 0..600 {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => last_error = Some(error),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    match last_error {
        Some(error) => Err(error).with_context(|| format!("timed out waiting for {description}")),
        None => bail!("timed out waiting for {description}"),
    }
}

async fn wait_until<F, Fut>(description: &str, mut condition: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    // Iroh discovery can be slower while the complete workspace suite is running its other
    // multi-peer tests concurrently.
    for _ in 0..600 {
        if condition().await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("timed out waiting for {description}")
}
