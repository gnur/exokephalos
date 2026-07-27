use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use xo_core::behavior::{Predicate, ViewDescriptor, WorkspaceBehavior};
use xo_core::iroh_node::{IrohNode, IrohWorkspace};
use xo_core::projection::ProjectionState;
use xo_core::records::{WorkspaceRecords, WorkspaceSnapshot};
use xo_core::sync_state::{Connectivity, SyncStateStore};
use xo_core::{
    ActorId, CURRENT_SCHEMA, DeviceRecord, HlcClock, Note, NoteId, NoteRevision, RevisionId,
};

const ACTIVE_WORKSPACE_FILE: &str = "active-workspace";

pub struct WorkspaceSession {
    node: IrohNode,
    workspace: IrohWorkspace,
    actor: ActorId,
    clock: HlcClock,
    projection: ProjectionState,
    pub sync_state: SyncStateStore,
}

impl WorkspaceSession {
    pub async fn open(
        state_dir: &Path,
        workspace_id: Option<&str>,
        ticket: Option<&str>,
        projection: PathBuf,
    ) -> Result<Self> {
        let node = IrohNode::persistent(state_dir).await?;
        let (workspace, reopened) =
            select_workspace(&node, state_dir, workspace_id, ticket).await?;
        if let Some(ticket) = ticket {
            workspace.start_sync(ticket).await?;
        } else if reopened {
            workspace.resume_sync().await?;
        }
        let actor = ActorId::new(workspace.author_id().to_string());
        let sync_state = SyncStateStore::open(state_dir.join("tui-sync.sqlite"))?;
        sync_state.set_connectivity(if ticket.is_some() || reopened {
            &Connectivity::Connecting
        } else {
            &Connectivity::Offline
        })?;
        Ok(Self {
            projection: ProjectionState::open(projection)?,
            node,
            workspace,
            actor: actor.clone(),
            clock: HlcClock::new(actor),
            sync_state,
        })
    }

    #[must_use]
    pub fn workspace_id(&self) -> String {
        self.workspace.id().to_string()
    }

    #[must_use]
    pub fn projection_root(&self) -> &Path {
        self.projection.root()
    }

    pub async fn snapshot(&self) -> Result<WorkspaceSnapshot> {
        let snapshot = WorkspaceRecords::new(&self.workspace).snapshot().await?;
        self.projection.reconcile(&snapshot.notes)?;
        self.projection.reconcile_assets(&snapshot.assets)?;
        self.projection.reconcile_configs(&snapshot.configs)?;
        Ok(snapshot)
    }

    pub async fn behavior(&mut self) -> Result<WorkspaceBehavior> {
        let records = WorkspaceRecords::new(&self.workspace);
        let configs = records.list_configs().await?;
        let mut modules = BTreeMap::new();
        let mut xo_main = None;
        for config in &configs {
            let source = String::from_utf8(config.bytes.clone())
                .with_context(|| format!("configuration {} is not UTF-8", config.record.path))?;
            match config.record.path.as_str() {
                "xo.scm" => xo_main = Some(source),
                _ => {
                    modules.insert(config.record.path.clone(), source);
                }
            }
        }
        let (mut behavior, upgrade_prerelease_config) = match xo_main {
            Some(source) => match xo_core::steel_runtime::SteelWorkspace::load(
                &source,
                &modules,
                "1970-01-01T00:00:00Z",
            ) {
                Ok(behavior) => (behavior, false),
                Err(error) => match modules
                    .is_empty()
                    .then_some(decode_prerelease_config(&source))
                    .flatten()
                {
                    Some(behavior) => (behavior?, true),
                    None => {
                        return Err(error)
                            .context("load replicated workspace configuration xo.scm");
                    }
                },
            },
            None => (WorkspaceBehavior::default(), false),
        };
        let install_default_views = behavior.views.is_empty();
        if install_default_views {
            behavior.default_view = "notes".into();
            behavior.views = default_views();
        }
        if upgrade_prerelease_config || install_default_views {
            let predecessors = configs
                .iter()
                .find(|config| config.record.path == "xo.scm")
                .map(|config| BTreeSet::from([config.revision_id.clone()]))
                .unwrap_or_default();
            records
                .put_config(
                    "xo.scm",
                    xo_core::steel_runtime::encode_config(&behavior, false).into_bytes(),
                    self.clock.next(now_ms()?),
                    predecessors,
                )
                .await?;
            self.projection
                .reconcile_configs(&records.list_configs().await?)?;
        }
        Ok(behavior)
    }

    pub async fn save(&mut self, note: &Note) -> Result<RevisionId> {
        self.commit(note, false).await
    }
    pub async fn delete(&mut self, note: &Note) -> Result<RevisionId> {
        self.commit(note, true).await
    }

    async fn commit(&mut self, note: &Note, deleted: bool) -> Result<RevisionId> {
        let records = WorkspaceRecords::new(&self.workspace);
        let mut predecessors = BTreeSet::new();
        if let Some(resolved) = records.load_note(&note.id).await? {
            predecessors.insert(resolved.winning_revision);
            predecessors.extend(
                resolved
                    .conflict
                    .into_iter()
                    .flat_map(|value| value.concurrent_revisions),
            );
        }
        records
            .commit_revision(&NoteRevision {
                schema: CURRENT_SCHEMA,
                note_id: note.id.clone(),
                frontmatter: note.frontmatter.clone(),
                body: note.body.clone(),
                materialized_path: xo_core::projection::canonical_note_path(
                    &note.id,
                    &note.frontmatter,
                ),
                hlc: self.clock.next(now_ms()?),
                author_id: self.actor.clone(),
                predecessors,
                deleted,
            })
            .await
            .map_err(Into::into)
    }

    pub fn refresh_sync(&self) -> Result<()> {
        self.sync_state.set_connectivity(&Connectivity::Direct)?;
        Ok(())
    }
    pub async fn writable_invitation(&self) -> Result<String> {
        self.workspace.share(true).await
    }
    pub async fn connect_peer(&self, ticket: &str) -> Result<()> {
        self.workspace.start_sync(ticket).await?;
        self.sync_state
            .set_connectivity(&Connectivity::Connecting)?;
        Ok(())
    }
    pub async fn deleted_notes(&self) -> Result<Vec<Note>> {
        Ok(WorkspaceRecords::new(&self.workspace)
            .deleted_notes()
            .await?)
    }
    pub async fn history(&self, note_id: &NoteId) -> Result<Vec<(RevisionId, NoteRevision)>> {
        Ok(WorkspaceRecords::new(&self.workspace)
            .revision_history(note_id)
            .await?)
    }
    pub async fn retire_device(&mut self, mut device: DeviceRecord) -> Result<()> {
        device.retired_at = Some(self.clock.next(now_ms()?));
        WorkspaceRecords::new(&self.workspace)
            .put_device(&device)
            .await?;
        Ok(())
    }
    pub fn retry(&self, operation_id: i64) -> Result<()> {
        self.sync_state.retry(operation_id)?;
        Ok(())
    }
    pub async fn shutdown(self) -> Result<()> {
        self.node.shutdown().await
    }
}

fn decode_prerelease_config(source: &str) -> Option<Result<WorkspaceBehavior>> {
    let encoded = source
        .trim()
        .strip_prefix("(workspace-config ")?
        .strip_suffix(')')?;
    let json = match serde_json::from_str::<String>(encoded) {
        Ok(json) => json,
        Err(error) => return Some(Err(error).context("decode prerelease workspace envelope")),
    };
    Some(
        serde_json::from_str::<WorkspaceBehavior>(&json)
            .context("decode prerelease workspace descriptor")
            .and_then(|behavior| {
                behavior
                    .validate()
                    .context("validate prerelease workspace descriptor")?;
                Ok(behavior)
            }),
    )
}

fn default_views() -> Vec<ViewDescriptor> {
    vec![
        ViewDescriptor {
            id: "notes".into(),
            name: "Notes".into(),
            key: Some("n".into()),
            show_tags: true,
            title_field: "title".into(),
            subtitle_field: None,
            sort_field: Some("created".into()),
            descending: true,
            preview: None,
            predicate: Predicate::FieldEquals {
                field: "type".into(),
                value: "note".into(),
            },
            subviews: vec![],
        },
        ViewDescriptor {
            id: "all".into(),
            name: "All".into(),
            key: Some("0".into()),
            show_tags: true,
            title_field: "title".into(),
            subtitle_field: Some("type".into()),
            sort_field: Some("created".into()),
            descending: true,
            preview: None,
            predicate: Predicate::Always,
            subviews: vec![],
        },
    ]
}

async fn select_workspace(
    node: &IrohNode,
    state_dir: &Path,
    requested: Option<&str>,
    ticket: Option<&str>,
) -> Result<(IrohWorkspace, bool)> {
    let (workspace, reopened) = if let Some(ticket) = ticket {
        (node.import_workspace(ticket).await?, false)
    } else if let Some(workspace_id) = requested {
        (
            node.open_workspace_str(workspace_id)
                .await?
                .context("workspace is not present in this peer")?,
            true,
        )
    } else if let Some(workspace) = open_active_workspace(node, state_dir).await? {
        (workspace, true)
    } else {
        let workspace_ids = node.workspace_ids().await?;
        match workspace_ids.as_slice() {
            [] => (node.create_workspace().await?, false),
            [workspace_id] => (
                node.open_workspace_str(workspace_id)
                    .await?
                    .context("the local workspace disappeared")?,
                true,
            ),
            _ => anyhow::bail!(
                "multiple local workspaces exist; choose one once with --workspace WORKSPACE_ID"
            ),
        }
    };
    std::fs::write(
        state_dir.join(ACTIVE_WORKSPACE_FILE),
        workspace.id().to_string(),
    )?;
    Ok((workspace, reopened))
}

async fn open_active_workspace(node: &IrohNode, state_dir: &Path) -> Result<Option<IrohWorkspace>> {
    let active = match std::fs::read_to_string(state_dir.join(ACTIVE_WORKSPACE_FILE)) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let active = active.trim();
    if active.is_empty() {
        return Ok(None);
    }
    node.open_workspace_str(active)
        .await?
        .with_context(|| format!("active workspace {active} is not present in this peer"))
        .map(Some)
}

fn now_ms() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch")?
        .as_millis()
        .try_into()
        .context("time does not fit u64")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use xo_core::domain::{Frontmatter, FrontmatterValue};
    use xo_core::iroh_node::IrohNode;
    use xo_core::records::WorkspaceRecords;
    use xo_core::{Hlc, NoteId};

    #[tokio::test]
    async fn fresh_local_start_creates_and_reopens_an_active_workspace() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let state = directory.path().join("state");
        let projection = directory.path().join("notes");
        let first = WorkspaceSession::open(&state, None, None, projection.clone()).await?;
        let workspace_id = first.workspace_id();
        assert_eq!(
            first.sync_state.status()?.connectivity,
            Connectivity::Offline
        );
        first.shutdown().await?;

        let reopened = WorkspaceSession::open(&state, None, None, projection).await?;
        assert_eq!(reopened.workspace_id(), workspace_id);
        assert_eq!(
            reopened.sync_state.status()?.connectivity,
            Connectivity::Connecting
        );
        assert_eq!(
            std::fs::read_to_string(state.join(ACTIVE_WORKSPACE_FILE))?,
            workspace_id
        );
        reopened.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn missing_views_create_default_xo_config_in_projection() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let state = directory.path().join("state");
        let projection = directory.path().join("notes");
        let mut session = WorkspaceSession::open(&state, None, None, projection.clone()).await?;

        let behavior = session.behavior().await?;

        assert_eq!(behavior.default_view, "notes");
        assert_eq!(
            behavior
                .views
                .iter()
                .map(|view| view.id.as_str())
                .collect::<Vec<_>>(),
            vec!["notes", "all"]
        );
        assert!(projection.join("xo.scm").is_file());
        let source = std::fs::read_to_string(projection.join("xo.scm"))?;
        assert!(source.starts_with("(workspace-config\n  (schema 1)"));
        assert!(source.contains("(field-equals \"type\" \"note\")"));
        assert!(!source.starts_with("(workspace-config \""));
        let configs = WorkspaceRecords::new(&session.workspace)
            .list_configs()
            .await?;
        assert!(configs.iter().any(|config| config.record.path == "xo.scm"));
        session.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn prerelease_json_workspace_config_is_upgraded_to_native_forms() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let state = directory.path().join("state");
        let projection = directory.path().join("notes");
        let mut session = WorkspaceSession::open(&state, None, None, projection.clone()).await?;
        let old_behavior = WorkspaceBehavior {
            default_view: "notes".into(),
            views: default_views(),
            ..WorkspaceBehavior::default()
        };
        let json = serde_json::to_string(&old_behavior)?;
        let envelope = format!("(workspace-config {})", serde_json::to_string(&json)?);
        WorkspaceRecords::new(&session.workspace)
            .put_config(
                "xo.scm",
                envelope.into_bytes(),
                session.clock.next(now_ms()?),
                BTreeSet::new(),
            )
            .await?;

        let behavior = session.behavior().await?;

        assert_eq!(behavior, old_behavior);
        let source = std::fs::read_to_string(projection.join("xo.scm"))?;
        assert!(source.starts_with("(workspace-config\n  (schema 1)"));
        assert!(!source.contains("(workspace-config \""));
        session.shutdown().await
    }

    #[tokio::test]
    async fn tui_pairing_invitation_connects_a_sync_peer() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut session = WorkspaceSession::open(
            &directory.path().join("client"),
            None,
            None,
            directory.path().join("projection"),
        )
        .await?;
        session.behavior().await?;
        let workspace_id = session.workspace_id();
        let client_ticket = session.writable_invitation().await?;

        let server = IrohNode::persistent(directory.path().join("server")).await?;
        let server_workspace = server.import_writable_workspace(&client_ticket).await?;
        assert_eq!(server_workspace.id().to_string(), workspace_id);
        let server_ticket = server_workspace.share(true).await?;

        session.connect_peer(&server_ticket).await?;
        assert_eq!(
            session.sync_state.status()?.connectivity,
            Connectivity::Connecting
        );
        server_workspace
            .put("pairing/verification", "connected")
            .await?;
        wait_until(|| async {
            session
                .workspace
                .get("pairing/verification")
                .await
                .ok()
                .flatten()
                .is_some()
        })
        .await?;

        session.shutdown().await?;
        server.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn offline_tui_edit_reconnects_retains_conflict_and_converges() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let primary_state = directory.path().join("primary");
        let central_state = directory.path().join("central");
        let primary = IrohNode::persistent(&primary_state).await?;
        let workspace = primary.create_workspace().await?;
        let primary_records = WorkspaceRecords::new(&workspace);
        let base = NoteRevision {
            schema: CURRENT_SCHEMA,
            note_id: NoteId::new("note002"),
            frontmatter: Frontmatter::from([(
                "title".into(),
                FrontmatterValue::String("Base".into()),
            )]),
            body: "base".into(),
            materialized_path: "notes/base.md".into(),
            hlc: Hlc {
                physical_ms: 100,
                logical: 0,
                actor_id: primary_records.actor_id(),
            },
            author_id: primary_records.actor_id(),
            predecessors: BTreeSet::new(),
            deleted: false,
        };
        let base_id = primary_records.commit_revision(&base).await?;
        let workspace_id = workspace.id().to_string();
        let ticket = workspace.share(true).await?;
        let central = IrohNode::persistent(&central_state).await?;
        let central_workspace = central.import_workspace(&ticket).await?;
        wait_until(|| async {
            WorkspaceRecords::new(&central_workspace)
                .get_revision(&base.note_id, &base_id)
                .await
                .ok()
                .flatten()
                .is_some()
        })
        .await?;
        central.shutdown().await?;
        primary.shutdown().await?;

        let mut session = WorkspaceSession::open(
            &primary_state,
            Some(&workspace_id),
            None,
            directory.path().join("projection"),
        )
        .await?;
        let mut offline_note = session.snapshot().await?.notes[0].clone();
        offline_note.body = "offline primary edit".into();
        session.save(&offline_note).await?;

        let central = IrohNode::persistent(&central_state).await?;
        let central_workspace = central
            .open_workspace_str(&workspace_id)
            .await?
            .context("central workspace missing")?;
        let central_records = WorkspaceRecords::new(&central_workspace);
        let central_actor = central_records.actor_id();
        central_records
            .commit_revision(&NoteRevision {
                schema: CURRENT_SCHEMA,
                note_id: base.note_id.clone(),
                frontmatter: base.frontmatter.clone(),
                body: "central concurrent edit".into(),
                materialized_path: base.materialized_path.clone(),
                hlc: Hlc {
                    physical_ms: 200,
                    logical: 0,
                    actor_id: central_actor.clone(),
                },
                author_id: central_actor,
                predecessors: BTreeSet::from([base_id]),
                deleted: false,
            })
            .await?;
        let primary_ticket = session.workspace.share(true).await?;
        let central_ticket = central_workspace.share(true).await?;
        central_workspace.start_sync(&primary_ticket).await?;
        session.workspace.start_sync(&central_ticket).await?;
        wait_until(|| async {
            session.snapshot().await.ok().is_some_and(|snapshot| {
                snapshot
                    .resolved
                    .iter()
                    .any(|value| value.conflict.is_some())
            }) && central_records
                .snapshot()
                .await
                .ok()
                .is_some_and(|snapshot| {
                    snapshot
                        .resolved
                        .iter()
                        .any(|value| value.conflict.is_some())
                })
        })
        .await?;
        let snapshot = session.snapshot().await?;
        assert_eq!(snapshot.notes.len(), 1);
        assert!(snapshot.resolved[0].conflict.is_some());
        assert!(session.history(&base.note_id).await?.len() >= 3);
        session.shutdown().await?;
        central.shutdown().await?;
        Ok(())
    }

    async fn wait_until<F, Fut>(mut condition: F) -> Result<()>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        for _ in 0..200 {
            if condition().await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        anyhow::bail!("peers did not converge before timeout")
    }
}
