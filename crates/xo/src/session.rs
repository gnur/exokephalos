use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use xo_core::behavior::{
    ActionDescriptor, ActionPlugin, Capability, Predicate, WorkspaceBehavior, default_views,
};
use xo_core::iroh_node::{IrohNode, IrohWorkspace, WorkspaceEvent};
use xo_core::projection::ProjectionState;
use xo_core::records::{WorkspaceRecords, WorkspaceSnapshot};
use xo_core::sync_state::{Connectivity, SyncStateStore};
use xo_core::{
    ActorId, CURRENT_SCHEMA, DeviceRecord, HlcClock, Note, NoteId, NoteRevision, RevisionId,
};

const ACTIVE_WORKSPACE_FILE: &str = "active-workspace";
const WORKSPACE_LOCK_FILE: &str = ".xo-workspace.lock";

struct WorkspaceLock {
    file: File,
}

impl WorkspaceLock {
    fn acquire(state_dir: &Path) -> Result<Self> {
        let path = state_dir.join(WORKSPACE_LOCK_FILE);
        let file = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open workspace lock {}", path.display()))?;
        file.try_lock_exclusive().map_err(|error| {
            anyhow::anyhow!(
                "workspace state is already in use by another xo process ({}): {error}",
                path.display()
            )
        })?;
        Ok(Self { file })
    }
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub struct WorkspaceSession {
    node: IrohNode,
    workspace: IrohWorkspace,
    actor: ActorId,
    clock: HlcClock,
    projection: ProjectionState,
    pub sync_state: SyncStateStore,
    membership: xo_core::MembershipIdentity,
    _lock: WorkspaceLock,
}

impl WorkspaceSession {
    pub async fn open(
        state_dir: &Path,
        workspace_id: Option<&str>,
        ticket: Option<&str>,
        projection: PathBuf,
    ) -> Result<Self> {
        let host = hostname::get()
            .context("read system hostname")?
            .into_string()
            .map_err(|_| anyhow::anyhow!("system hostname is not valid UTF-8"))?;
        let peer_id = xo_core::PeerId::parse(host).context("validate host peer ID")?;
        Self::open_with_peer(state_dir, workspace_id, ticket, projection, peer_id).await
    }

    pub async fn open_with_peer(
        state_dir: &Path,
        workspace_id: Option<&str>,
        ticket: Option<&str>,
        projection: PathBuf,
        peer_id: xo_core::PeerId,
    ) -> Result<Self> {
        std::fs::create_dir_all(state_dir)
            .with_context(|| format!("create state directory {}", state_dir.display()))?;
        let lock = WorkspaceLock::acquire(state_dir)?;
        let membership = xo_core::membership::load_or_create_identity(state_dir, &peer_id)?;
        let node = IrohNode::persistent_with_peer(state_dir, peer_id).await?;
        let (workspace, reopened) =
            select_workspace(&node, state_dir, workspace_id, ticket).await?;
        if let Some(ticket) = ticket {
            workspace.start_sync(ticket).await?;
        } else if reopened {
            workspace.resume_sync().await?;
        }
        let actor = ActorId::new(workspace.author_id().to_string());
        let clock = HlcClock::new(actor.clone());
        WorkspaceRecords::new(&workspace)
            .put_device(&DeviceRecord {
                schema: CURRENT_SCHEMA,
                endpoint_id: node.endpoint_id().to_string(),
                author_id: actor.clone(),
                label: membership.peer_id().to_string(),
                capabilities: BTreeSet::from(["write".to_owned(), "tui".to_owned()]),
                last_seen_ms: Some(now_ms()?),
                retired_at: None,
            })
            .await?;
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
            clock,
            sync_state,
            membership,
            _lock: lock,
        })
    }

    #[must_use]
    pub fn peer_id(&self) -> &xo_core::PeerId {
        self.membership.peer_id()
    }

    #[must_use]
    pub fn membership_fingerprint(&self) -> String {
        self.membership.fingerprint()
    }

    pub async fn pending_membership_requests(&self) -> Vec<xo_core::peer_protocol::JoinRequest> {
        self.workspace.pending_requests().await
    }

    pub async fn members(&self) -> Vec<xo_core::membership::Member> {
        self.workspace.members().await
    }

    pub async fn approve_member(&self, public_key: &[u8; 32]) -> Result<()> {
        self.workspace.approve_peer(public_key).await
    }

    pub async fn reject_member(&self, public_key: &[u8; 32]) -> Result<()> {
        self.workspace.reject_peer(public_key).await
    }

    pub async fn remove_member(&self, public_key: &[u8; 32]) -> Result<()> {
        self.workspace
            .remove_peer(public_key, Some("removed from xo TUI".to_owned()))
            .await
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
        self.projection
            .reconcile_projection_configs(&snapshot.configs)?;
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
        let had_workspace_config = xo_main.is_some();
        let mut behavior = match xo_main {
            Some(source) => xo_core::steel_runtime::SteelWorkspace::load(
                &source,
                &modules,
                "1970-01-01T00:00:00+00:00",
            )
            .context("load replicated workspace configuration xo.scm")?,
            None => WorkspaceBehavior::default(),
        };
        let install_default_views = behavior.views.is_empty();
        if install_default_views {
            behavior.default_view = "notes".into();
            behavior.views = default_views();
        }
        let install_url_capture = !had_workspace_config
            && !behavior
                .actions
                .iter()
                .any(|action| action.id == "capture-url");
        if install_url_capture {
            behavior.actions.push(ActionDescriptor {
                id: "capture-url".into(),
                description: "Capture readable content from a URL".into(),
                predicate: Predicate::Always,
                effects: vec![],
                plugin: Some(ActionPlugin::CaptureUrl),
            });
            behavior.capability_grants.insert(
                "capture-url".into(),
                BTreeSet::from([Capability::CreateNote, Capability::Network]),
            );
        }
        if install_default_views || install_url_capture {
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
                .reconcile_projection_configs(&records.list_configs().await?)?;
        }
        Ok(behavior)
    }

    pub async fn workspace_config_source(&self) -> Result<String> {
        let config = WorkspaceRecords::new(&self.workspace)
            .list_configs()
            .await?
            .into_iter()
            .find(|config| config.record.path == "xo.scm")
            .context("workspace configuration is unavailable")?;
        String::from_utf8(config.bytes).context("workspace configuration is not UTF-8")
    }

    pub async fn save_workspace_config(&mut self, source: &str) -> Result<()> {
        let configs = WorkspaceRecords::new(&self.workspace)
            .list_configs()
            .await?;
        let mut modules = BTreeMap::new();
        for config in &configs {
            if config.record.path != "xo.scm" {
                modules.insert(
                    config.record.path.clone(),
                    String::from_utf8(config.bytes.clone())?,
                );
            }
        }
        xo_core::steel_runtime::SteelWorkspace::load(source, &modules, "1970-01-01T00:00:00+00:00")
            .context("validate workspace configuration")?;
        let predecessors = configs
            .iter()
            .find(|config| config.record.path == "xo.scm")
            .map(|config| BTreeSet::from([config.revision_id.clone()]))
            .unwrap_or_default();
        WorkspaceRecords::new(&self.workspace)
            .put_config(
                "xo.scm",
                source.as_bytes().to_vec(),
                self.clock.next(now_ms()?),
                predecessors,
            )
            .await?;
        Ok(())
    }

    pub async fn install_config(&mut self, path: &str, source: &[u8]) -> Result<()> {
        if !path.starts_with("plugins/") || !xo_core::steel_runtime::valid_config_path(path) {
            anyhow::bail!("plugin path must be below plugins/ and end in .scm");
        }
        let records = WorkspaceRecords::new(&self.workspace);
        let configs = records.list_configs().await?;
        let predecessors = configs
            .iter()
            .find(|config| config.record.path == path)
            .map(|config| BTreeSet::from([config.revision_id.clone()]))
            .unwrap_or_default();
        records
            .put_config(
                path,
                source.to_vec(),
                self.clock.next(now_ms()?),
                predecessors,
            )
            .await?;
        self.projection
            .reconcile_configs(&records.list_configs().await?)?;
        Ok(())
    }

    pub async fn config_source(&self, path: &str) -> Result<String> {
        let config = WorkspaceRecords::new(&self.workspace)
            .list_configs()
            .await?
            .into_iter()
            .find(|config| config.record.path == path)
            .with_context(|| format!("plugin configuration {path} is unavailable"))?;
        String::from_utf8(config.bytes)
            .with_context(|| format!("plugin configuration {path} is not UTF-8"))
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

    pub async fn subscribe(
        &self,
    ) -> Result<impl futures_lite::Stream<Item = Result<WorkspaceEvent>> + Send + Unpin + 'static>
    {
        self.workspace.subscribe().await
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
    async fn missing_views_create_default_xo_config_without_projection_file() -> Result<()> {
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
        assert!(!projection.join("xo.scm").exists());
        let source = session.workspace_config_source().await?;
        assert!(source.starts_with("(workspace-config\n  (schema 1)"));
        assert!(source.contains("(field-equals \"type\" \"note\")"));
        assert!(source.contains("(plugin (capture-url))"));
        assert!(source.contains("(capabilities create-note network)"));
        assert!(!source.starts_with("(workspace-config \""));
        let configs = WorkspaceRecords::new(&session.workspace)
            .list_configs()
            .await?;
        assert!(configs.iter().any(|config| config.record.path == "xo.scm"));
        session.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn bundled_plugin_install_is_replicated_projected_and_loaded() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let state = directory.path().join("state");
        let projection = directory.path().join("notes");
        let mut session = WorkspaceSession::open(&state, None, None, projection.clone()).await?;
        session.behavior().await?;
        session
            .install_config(
                "plugins/hardcover.scm",
                include_bytes!("../../../plugins/hardcover.scm"),
            )
            .await?;

        let behavior = session.behavior().await?;
        assert!(
            behavior
                .actions
                .iter()
                .any(|action| action.id == "hardcover-search")
        );
        assert!(projection.join("plugins/hardcover.scm").is_file());
        assert!(
            WorkspaceRecords::new(&session.workspace)
                .list_configs()
                .await?
                .iter()
                .any(|config| config.record.path == "plugins/hardcover.scm")
        );
        session.shutdown().await?;
        Ok(())
    }

    fn library_behavior() -> WorkspaceBehavior {
        WorkspaceBehavior {
            default_view: "library".into(),
            views: vec![xo_core::behavior::ViewDescriptor {
                id: "library".into(),
                name: "Library".into(),
                key: None,
                show_tags: true,
                title_field: "title".into(),
                subtitle_field: Some("type".into()),
                sort_field: Some("title".into()),
                descending: false,
                preview: None,
                predicate: Predicate::FieldEquals {
                    field: "type".into(),
                    value: "book".into(),
                },
                subviews: vec![xo_core::behavior::SubviewDescriptor {
                    id: "reading".into(),
                    name: "Reading".into(),
                    predicate: Predicate::HasTag {
                        tag: "reading".into(),
                    },
                }],
            }],
            ..WorkspaceBehavior::default()
        }
    }

    fn reading_book() -> Note {
        Note {
            id: NoteId::new("bkabcde"),
            path: "books/bkabcde.md".into(),
            frontmatter: Frontmatter::from([
                ("id".into(), FrontmatterValue::String("bkabcde".into())),
                (
                    "title".into(),
                    FrontmatterValue::String("TUI reading fixture".into()),
                ),
                ("type".into(), FrontmatterValue::String("book".into())),
                (
                    "tags".into(),
                    FrontmatterValue::Sequence(vec![FrontmatterValue::String("reading".into())]),
                ),
            ]),
            body: "created by the native TUI peer".into(),
        }
    }

    #[tokio::test]
    async fn tui_peer_receives_replicated_views_subviews_and_items() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut source = WorkspaceSession::open_with_peer(
            &directory.path().join("source-state"),
            None,
            None,
            directory.path().join("source-notes"),
            xo_core::PeerId::parse("source")?,
        )
        .await?;
        let behavior = library_behavior();
        WorkspaceRecords::new(&source.workspace)
            .put_config(
                "xo.scm",
                xo_core::steel_runtime::encode_config(&behavior, false).into_bytes(),
                source.clock.next(now_ms()?),
                BTreeSet::new(),
            )
            .await?;
        source.save(&reading_book()).await?;
        let ticket = source.writable_invitation().await?;

        let peer_state = directory.path().join("peer-state");
        assert!(
            WorkspaceSession::open_with_peer(
                &peer_state,
                None,
                Some(&ticket),
                directory.path().join("peer-notes"),
                xo_core::PeerId::parse("peer")?,
            )
            .await
            .is_err()
        );
        let request = source.workspace.pending_requests().await.remove(0);
        source.workspace.approve_peer(&request.public_key).await?;
        let mut peer = WorkspaceSession::open_with_peer(
            &peer_state,
            None,
            Some(&ticket),
            directory.path().join("peer-notes"),
            xo_core::PeerId::parse("peer")?,
        )
        .await?;
        let mut replicated_snapshot = None;
        for _ in 0..200 {
            if let Ok(snapshot) = peer.snapshot().await
                && snapshot
                    .configs
                    .iter()
                    .any(|config| config.record.path == "xo.scm")
                && snapshot
                    .notes
                    .iter()
                    .any(|note| note.id.as_str() == "bkabcde")
            {
                replicated_snapshot = Some(snapshot);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let snapshot = replicated_snapshot.context("TUI peer did not receive note and config")?;
        let replicated = peer.behavior().await?;
        assert_eq!(replicated.default_view, "library");
        assert_eq!(replicated.views[0].subviews[0].id, "reading");
        let matches = replicated.query(
            &snapshot.notes,
            &xo_core::behavior::Query {
                view: "library".into(),
                subview: Some("reading".into()),
                ..xo_core::behavior::Query::default()
            },
        )?;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id.as_str(), "bkabcde");

        let mut events = peer.subscribe().await?;
        let mut live_note = reading_book();
        live_note.id = NoteId::new("bkabcdf");
        live_note.path = "books/bkabcdf.md".into();
        live_note
            .frontmatter
            .insert("id".into(), FrontmatterValue::String("bkabcdf".into()));
        source.save(&live_note).await?;
        tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(event) = futures_lite::StreamExt::next(&mut events).await {
                if event? == WorkspaceEvent::ContentChanged
                    && peer.snapshot().await.is_ok_and(|snapshot| {
                        snapshot
                            .notes
                            .iter()
                            .any(|note| note.id.as_str() == "bkabcdf")
                    })
                {
                    return Ok::<_, anyhow::Error>(());
                }
            }
            anyhow::bail!("workspace event stream ended before the live update")
        })
        .await
        .context("TUI peer did not receive a live workspace event")??;
        peer.shutdown().await?;
        source.shutdown().await
    }

    #[tokio::test]
    async fn tui_pairing_invitation_connects_a_sync_peer() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut session = WorkspaceSession::open_with_peer(
            &directory.path().join("client"),
            None,
            None,
            directory.path().join("projection"),
            xo_core::PeerId::parse("client")?,
        )
        .await?;
        session.behavior().await?;
        let workspace_id = session.workspace_id();
        let client_ticket = session.writable_invitation().await?;

        let server = IrohNode::persistent_with_peer(
            directory.path().join("server"),
            xo_core::PeerId::parse("server")?,
        )
        .await?;
        assert!(
            server
                .import_writable_workspace(&client_ticket)
                .await
                .is_err()
        );
        let request = session.workspace.pending_requests().await.remove(0);
        session.workspace.approve_peer(&request.public_key).await?;
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
        let primary =
            IrohNode::persistent_with_peer(&primary_state, xo_core::PeerId::parse("primary")?)
                .await?;
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
        let central =
            IrohNode::persistent_with_peer(&central_state, xo_core::PeerId::parse("central")?)
                .await?;
        assert!(central.import_workspace(&ticket).await.is_err());
        let request = workspace.pending_requests().await.remove(0);
        workspace.approve_peer(&request.public_key).await?;
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

        let mut session = WorkspaceSession::open_with_peer(
            &primary_state,
            Some(&workspace_id),
            None,
            directory.path().join("projection"),
            xo_core::PeerId::parse("primary")?,
        )
        .await?;
        let mut offline_note = session.snapshot().await?.notes[0].clone();
        offline_note.body = "offline primary edit".into();
        session.save(&offline_note).await?;

        let central =
            IrohNode::persistent_with_peer(&central_state, xo_core::PeerId::parse("central")?)
                .await?;
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
