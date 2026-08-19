use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use xo_core::behavior::{
    ActionDescriptor, ActionPlugin, Capability, Predicate, WorkspaceBehavior, default_views,
};
use xo_core::projection::ProjectionState;
use xo_core::records::{WorkspaceRecords, WorkspaceSnapshot};
use xo_core::sync_state::{Connectivity, SyncStateStore};
use xo_core::{ActorId, CURRENT_SCHEMA, HlcClock, Note, NoteId, NoteRevision, RevisionId};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceEvent {
    ContentChanged,
    StatusChanged,
}

pub struct WorkspaceSession {
    replica: std::sync::Arc<xo_core::central_replica::CentralReplica>,
    client: Option<crate::central_client::CentralClient>,
    client_id: xo_core::ClientId,
    actor: ActorId,
    clock: HlcClock,
    projection: ProjectionState,
    pub sync_state: SyncStateStore,
    plugin_sources: BTreeMap<String, String>,
    _lock: WorkspaceLock,
}

impl WorkspaceSession {
    pub fn open(state_dir: &Path, workspace_id: Option<&str>, projection: PathBuf) -> Result<Self> {
        let host = hostname::get()
            .context("read system hostname")?
            .into_string()
            .map_err(|_| anyhow::anyhow!("system hostname is not valid UTF-8"))?;
        let client_id = xo_core::ClientId::parse(host).context("validate host client ID")?;
        let workspace_id = match workspace_id {
            Some(value) => value.to_owned(),
            None => read_active_workspace(state_dir)?.unwrap_or_else(local_workspace_id),
        };
        Self::build(
            state_dir,
            &workspace_id,
            projection,
            client_id,
            None,
            BTreeMap::new(),
        )
    }

    pub async fn open_central(
        state_dir: &Path,
        server: &str,
        projection: PathBuf,
        client_id: xo_core::ClientId,
    ) -> Result<Self> {
        Self::open_central_with_plugins(state_dir, server, projection, client_id, BTreeMap::new())
            .await
    }

    pub async fn open_central_with_plugins(
        state_dir: &Path,
        server: &str,
        projection: PathBuf,
        client_id: xo_core::ClientId,
        plugin_sources: BTreeMap<String, String>,
    ) -> Result<Self> {
        let workspace_id = match read_active_workspace(state_dir)? {
            Some(value) => value,
            None => {
                crate::central_client::CentralClient::discover_workspace(server, client_id.as_str())
                    .await
                    .context("discover server workspace")?
            }
        };
        Self::build(
            state_dir,
            &workspace_id,
            projection,
            client_id,
            Some(server),
            plugin_sources,
        )
    }

    fn build(
        state_dir: &Path,
        workspace_id: &str,
        projection: PathBuf,
        client_id: xo_core::ClientId,
        server: Option<&str>,
        plugin_sources: BTreeMap<String, String>,
    ) -> Result<Self> {
        std::fs::create_dir_all(state_dir)
            .with_context(|| format!("create state directory {}", state_dir.display()))?;
        let lock = WorkspaceLock::acquire(state_dir)?;
        std::fs::write(state_dir.join(ACTIVE_WORKSPACE_FILE), workspace_id)?;
        let automerge_actor = load_or_create_actor(state_dir)?;
        let actor = ActorId::new(blake3::hash(&automerge_actor).to_hex().to_string());
        let replica = xo_core::central_replica::CentralReplica::open(
            state_dir,
            workspace_id,
            actor.clone(),
            &automerge_actor,
        )?;
        let client = server
            .map(|server| {
                crate::central_client::CentralClient::start(
                    server,
                    client_id.to_string(),
                    std::sync::Arc::clone(&replica),
                )
            })
            .transpose()?;
        let sync_state = SyncStateStore::open(state_dir.join("tui-sync.sqlite"))?;
        sync_state.set_connectivity(if client.is_some() {
            &Connectivity::Connecting
        } else {
            &Connectivity::Offline
        })?;
        Ok(Self {
            projection: ProjectionState::open(projection)?,
            replica,
            client,
            client_id,
            actor: actor.clone(),
            clock: HlcClock::new(actor),
            sync_state,
            plugin_sources,
            _lock: lock,
        })
    }

    #[must_use]
    pub fn client_id(&self) -> &str {
        self.client_id.as_str()
    }

    pub async fn connected_clients(&self) -> Vec<String> {
        self.replica.connected_clients().await
    }

    #[must_use]
    pub fn workspace_id(&self) -> String {
        self.replica.workspace_id().to_owned()
    }

    #[must_use]
    pub fn projection_root(&self) -> &Path {
        self.projection.root()
    }

    pub async fn snapshot(&self) -> Result<WorkspaceSnapshot> {
        self.update_connectivity()?;
        let snapshot = WorkspaceRecords::new(self.replica.as_ref())
            .snapshot()
            .await?;
        self.projection.reconcile(&snapshot.notes)?;
        self.projection.reconcile_assets(&snapshot.assets)?;
        self.projection
            .reconcile_projection_configs(&snapshot.configs)?;
        Ok(snapshot)
    }

    pub async fn behavior(&mut self) -> Result<WorkspaceBehavior> {
        let records = WorkspaceRecords::new(self.replica.as_ref());
        let configs = records.list_configs().await?;
        let mut modules = BTreeMap::new();
        let mut xo_main = None;
        for config in &configs {
            let source = String::from_utf8(config.bytes.clone())
                .with_context(|| format!("configuration {} is not UTF-8", config.record.path))?;
            match config.record.path.as_str() {
                "xo.scm" => xo_main = Some(source),
                _ if xo_core::steel_runtime::valid_module_path(&config.record.path) => {
                    modules.insert(config.record.path.clone(), source);
                }
                _ if xo_core::steel_runtime::valid_plugin_path(&config.record.path) => {
                    // Plugins are local xo configuration, never replicated workspace state.
                }
                _ => anyhow::bail!(
                    "invalid workspace configuration path {}",
                    config.record.path
                ),
            }
        }
        let had_workspace_config = xo_main.is_some();
        let mut behavior = match xo_main {
            Some(source) => xo_core::steel_runtime::SteelWorkspace::load_with_plugins(
                &source,
                &modules,
                &self.plugin_sources,
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
        if !had_workspace_config && !self.plugin_sources.is_empty() {
            behavior = xo_core::steel_runtime::SteelWorkspace::load_with_plugins(
                &xo_core::steel_runtime::encode_config(&behavior, false),
                &BTreeMap::new(),
                &self.plugin_sources,
                "1970-01-01T00:00:00+00:00",
            )
            .context("load local Forge plugins")?;
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
            let mut persisted_behavior = behavior.clone();
            persisted_behavior.actions.retain(|action| {
                !matches!(
                    action.plugin,
                    Some(ActionPlugin::Steel { .. } | ActionPlugin::TagPicker)
                )
            });
            persisted_behavior.capability_grants.retain(|action, _| {
                persisted_behavior
                    .actions
                    .iter()
                    .any(|value| &value.id == action)
            });
            records
                .put_config(
                    "xo.scm",
                    xo_core::steel_runtime::encode_config(&persisted_behavior, false).into_bytes(),
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
        let config = WorkspaceRecords::new(self.replica.as_ref())
            .list_configs()
            .await?
            .into_iter()
            .find(|config| config.record.path == "xo.scm")
            .context("workspace configuration is unavailable")?;
        String::from_utf8(config.bytes).context("workspace configuration is not UTF-8")
    }

    pub async fn save_workspace_config(&mut self, source: &str) -> Result<()> {
        let configs = WorkspaceRecords::new(self.replica.as_ref())
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
        WorkspaceRecords::new(self.replica.as_ref())
            .put_config(
                "xo.scm",
                source.as_bytes().to_vec(),
                self.clock.next(now_ms()?),
                predecessors,
            )
            .await?;
        Ok(())
    }

    pub async fn config_source(&self, path: &str) -> Result<String> {
        if let Some(source) = self.plugin_sources.get(path) {
            return Ok(source.clone());
        }
        let config = WorkspaceRecords::new(self.replica.as_ref())
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
        let records = WorkspaceRecords::new(self.replica.as_ref());
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

    pub fn subscribe(
        &self,
    ) -> Result<
        std::pin::Pin<
            Box<dyn futures_lite::Stream<Item = Result<WorkspaceEvent>> + Send + 'static>,
        >,
    > {
        let receiver = self.replica.subscribe();
        Ok(Box::pin(futures_lite::stream::unfold(
            receiver,
            |mut receiver| async move {
                match receiver.recv().await {
                    Ok(xo_core::central_replica::ReplicaEvent::ContentChanged) => {
                        Some((Ok(WorkspaceEvent::ContentChanged), receiver))
                    }
                    Ok(xo_core::central_replica::ReplicaEvent::StatusChanged)
                    | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        Some((Ok(WorkspaceEvent::StatusChanged), receiver))
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
                }
            },
        )))
    }

    pub fn refresh_sync(&self) -> Result<()> {
        self.update_connectivity()
    }

    fn update_connectivity(&self) -> Result<()> {
        let connectivity = match self
            .client
            .as_ref()
            .map(crate::central_client::CentralClient::status)
        {
            Some(crate::central_client::CentralClientStatus::Connected) => Connectivity::Connected,
            Some(crate::central_client::CentralClientStatus::Connecting) => {
                Connectivity::Connecting
            }
            Some(
                crate::central_client::CentralClientStatus::Offline(_)
                | crate::central_client::CentralClientStatus::Stopped,
            )
            | None => Connectivity::Offline,
        };
        self.sync_state.set_connectivity(&connectivity)?;
        Ok(())
    }
    pub async fn deleted_notes(&self) -> Result<Vec<Note>> {
        Ok(WorkspaceRecords::new(self.replica.as_ref())
            .deleted_notes()
            .await?)
    }
    pub async fn history(&self, note_id: &NoteId) -> Result<Vec<(RevisionId, NoteRevision)>> {
        Ok(WorkspaceRecords::new(self.replica.as_ref())
            .revision_history(note_id)
            .await?)
    }
    pub fn retry(&self, operation_id: i64) -> Result<()> {
        self.sync_state.retry(operation_id)?;
        Ok(())
    }
    pub async fn shutdown(self) -> Result<()> {
        if let Some(client) = self.client {
            client.shutdown().await?;
        }
        Ok(())
    }
}

fn read_active_workspace(state_dir: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(state_dir.join(ACTIVE_WORKSPACE_FILE)) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => Ok(Some(value.trim().to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn local_workspace_id() -> String {
    let random = rand::random::<[u8; 16]>();
    format!("local-{}", &blake3::hash(&random).to_hex()[..24])
}

fn load_or_create_actor(state_dir: &Path) -> Result<[u8; 32]> {
    let path = state_dir.join("automerge-actor");
    match std::fs::read(&path) {
        Ok(bytes) => bytes.try_into().map_err(|_| {
            anyhow::anyhow!(
                "Automerge actor {} must contain exactly 32 bytes",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let bytes = rand::random::<[u8; 32]>();
            std::fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
            Ok(bytes)
        }
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
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

    #[tokio::test]
    async fn local_central_replica_reopens_without_a_network() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let state = directory.path().join("state");
        let projection = directory.path().join("notes");
        let mut first = WorkspaceSession::open(&state, None, projection.clone())?;
        let workspace_id = first.workspace_id();
        assert_eq!(
            first.sync_state.status()?.connectivity,
            Connectivity::Offline
        );
        assert_eq!(first.behavior().await?.default_view, "notes");
        first.shutdown().await?;

        let reopened = WorkspaceSession::open(&state, None, projection)?;
        assert_eq!(reopened.workspace_id(), workspace_id);
        reopened.shutdown().await
    }
}
