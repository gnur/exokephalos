//! Typed exokephalos records stored in an Iroh Docs workspace.

use std::collections::BTreeMap;

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::iroh_node::IrohWorkspace;
use crate::local_index::{IndexError, LocalIndex};
use crate::projection::{Diagnostic, ProjectedAsset};
use crate::{
    ActorId, AssetId, AssetRecord, CURRENT_SCHEMA, ConfigRevision, DeviceRecord, DomainError, Head,
    Hlc, Note, NoteId, NoteRevision, ResolvedNote, RevisionGraphError, RevisionId, Tombstone,
    WorkspaceDescriptor, resolve_heads, validate_revision_graph,
};

const NOTE_PREFIX: &str = "note/";
const ASSET_PREFIX: &str = "asset/";
const ASSET_BLOB_PREFIX: &str = "asset-blob/";
const CONFIG_PREFIX: &str = "config/";
const CONFIG_BLOB_PREFIX: &str = "config-blob/";
const TOMBSTONE_PREFIX: &str = "tombstone/";
const DEVICE_PREFIX: &str = "device/";
const WORKSPACE_DESCRIPTOR_KEY: &str = "workspace/descriptor";

#[derive(Debug, Error)]
pub enum RecordError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Graph(#[from] RevisionGraphError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error("Iroh record transport failed: {0}")]
    Transport(#[from] anyhow::Error),
    #[error("invalid record key: {0}")]
    InvalidKey(String),
    #[error("record encoding failed: {0}")]
    Encoding(String),
    #[error("record decoding failed: {0}")]
    Decoding(String),
    #[error("head references an unavailable revision: {0}")]
    MissingRevision(RevisionId),
    #[error("record key does not match its value: {0}")]
    KeyMismatch(String),
    #[error("asset blob is unavailable: {0}")]
    MissingBlob(AssetId),
    #[error("asset blob does not match its record: {0}")]
    BlobMismatch(AssetId),
    #[error("referenced content is unavailable: {0}")]
    MissingContent(String),
    #[error("referenced content does not match its record: {0}")]
    ContentMismatch(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedConfig {
    pub revision_id: RevisionId,
    pub record: ConfigRevision,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkspaceSnapshot {
    pub notes: Vec<Note>,
    pub resolved: Vec<ResolvedNote>,
    pub assets: Vec<ProjectedAsset>,
    pub configs: Vec<ProjectedConfig>,
    pub tombstones: Vec<Tombstone>,
    pub devices: Vec<DeviceRecord>,
    pub descriptor: Option<WorkspaceDescriptor>,
    pub diagnostics: Vec<Diagnostic>,
}

/// The authoritative revision and per-author head repository for a workspace.
#[derive(Clone, Copy, Debug)]
pub struct WorkspaceRecords<'a> {
    workspace: &'a IrohWorkspace,
}

impl<'a> WorkspaceRecords<'a> {
    #[must_use]
    pub const fn new(workspace: &'a IrohWorkspace) -> Self {
        Self { workspace }
    }

    #[must_use]
    pub fn actor_id(&self) -> ActorId {
        ActorId::new(self.workspace.author_id().to_string())
    }

    pub async fn put_revision(&self, revision: &NoteRevision) -> Result<RevisionId, RecordError> {
        let revision_id = revision.id()?;
        self.workspace
            .put(
                revision_key(&revision.note_id, &revision_id),
                revision.canonical_bytes()?,
            )
            .await?;
        Ok(revision_id)
    }

    pub async fn set_head(&self, head: &Head) -> Result<(), RecordError> {
        head.validate()?;
        if self
            .workspace
            .get(revision_key(&head.note_id, &head.revision_id))
            .await?
            .is_none()
        {
            return Err(RecordError::MissingRevision(head.revision_id.clone()));
        }
        self.workspace
            .put(head_key(&head.note_id, &head.author_id), encode(head)?)
            .await?;
        Ok(())
    }

    /// Store an immutable revision, then advance that revision author's head.
    pub async fn commit_revision(
        &self,
        revision: &NoteRevision,
    ) -> Result<RevisionId, RecordError> {
        let revision_id = self.put_revision(revision).await?;
        self.set_head(&Head {
            note_id: revision.note_id.clone(),
            author_id: revision.author_id.clone(),
            revision_id: revision_id.clone(),
        })
        .await?;
        Ok(revision_id)
    }

    pub async fn load_note(&self, note_id: &NoteId) -> Result<Option<ResolvedNote>, RecordError> {
        let prefix = format!("{NOTE_PREFIX}{note_id}/");
        let mut group = NoteRecords::default();
        for (key, value) in self.workspace.list(prefix).await? {
            let key = String::from_utf8(key).map_err(|error| {
                RecordError::InvalidKey(String::from_utf8_lossy(error.as_bytes()).into_owned())
            })?;
            decode_record(&key, &value, Some(note_id), &mut group)?;
        }
        resolve_group(&group)
    }

    pub async fn get_revision(
        &self,
        note_id: &NoteId,
        revision_id: &RevisionId,
    ) -> Result<Option<NoteRevision>, RecordError> {
        let Some(bytes) = self
            .workspace
            .get(revision_key(note_id, revision_id))
            .await?
        else {
            return Ok(None);
        };
        let revision: NoteRevision = decode(&bytes)?;
        revision.validate()?;
        if revision.note_id != *note_id || revision.id()? != *revision_id {
            return Err(RecordError::KeyMismatch(revision_key(note_id, revision_id)));
        }
        Ok(Some(revision))
    }

    /// Store verified asset bytes and publish their typed metadata record.
    pub async fn put_asset(
        &self,
        id: AssetId,
        mime: impl Into<String>,
        materialized_path: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Result<AssetRecord, RecordError> {
        let size = u64::try_from(bytes.len())
            .map_err(|_| RecordError::Encoding("asset size exceeds u64".to_owned()))?;
        let blob_hash = self.workspace.put_blob(asset_blob_key(&id), bytes).await?;
        let record = AssetRecord {
            schema: CURRENT_SCHEMA,
            id,
            blob_hash,
            mime: mime.into(),
            size,
            materialized_path: materialized_path.into(),
        };
        record.validate()?;
        self.workspace
            .put(asset_key(&record.id), encode(&record)?)
            .await?;
        Ok(record)
    }

    pub async fn get_asset(&self, id: &AssetId) -> Result<Option<ProjectedAsset>, RecordError> {
        let Some(record_bytes) = self.workspace.get(asset_key(id)).await? else {
            return Ok(None);
        };
        let record: AssetRecord = decode(&record_bytes)?;
        record.validate()?;
        if record.id != *id {
            return Err(RecordError::KeyMismatch(asset_key(id)));
        }
        let bytes = self
            .workspace
            .get(asset_blob_key(id))
            .await?
            .ok_or_else(|| RecordError::MissingBlob(id.clone()))?;
        let size_matches = u64::try_from(bytes.len()).ok() == Some(record.size);
        let hash_matches = blake3::hash(&bytes).to_hex().as_str() == record.blob_hash;
        if !size_matches || !hash_matches {
            return Err(RecordError::BlobMismatch(id.clone()));
        }
        Ok(Some(ProjectedAsset { record, bytes }))
    }

    pub async fn list_assets(&self) -> Result<Vec<ProjectedAsset>, RecordError> {
        let mut assets = Vec::new();
        for (key, _) in self.workspace.list(ASSET_PREFIX).await? {
            let key = String::from_utf8_lossy(&key);
            let parts = key.split('/').collect::<Vec<_>>();
            if parts.len() != 2 || parts[0] != "asset" || parts[1].is_empty() {
                continue;
            }
            let id = AssetId::new(parts[1]);
            if let Some(asset) = self.get_asset(&id).await? {
                assets.push(asset);
            }
        }
        assets.sort_by(|left, right| left.record.id.cmp(&right.record.id));
        Ok(assets)
    }

    pub async fn put_config(
        &self,
        path: impl Into<String>,
        bytes: Vec<u8>,
        hlc: Hlc,
        predecessors: std::collections::BTreeSet<RevisionId>,
    ) -> Result<ProjectedConfig, RecordError> {
        let path = path.into();
        let size = u64::try_from(bytes.len())
            .map_err(|_| RecordError::Encoding("configuration size exceeds u64".to_owned()))?;
        let content_identity = blake3::hash(&bytes).to_hex().to_string();
        let record = ConfigRevision {
            schema: CURRENT_SCHEMA,
            path,
            blob_hash: content_identity,
            size,
            author_id: hlc.actor_id.clone(),
            hlc,
            predecessors,
        };
        let revision_id = record.id()?;
        let stored_hash = self
            .workspace
            .put_blob(config_blob_key(&revision_id), bytes.clone())
            .await?;
        if stored_hash != record.blob_hash {
            return Err(RecordError::ContentMismatch(record.path));
        }
        self.workspace
            .put(config_key(&record, &revision_id), encode(&record)?)
            .await?;
        Ok(ProjectedConfig {
            revision_id,
            record,
            bytes,
        })
    }

    pub async fn list_configs(&self) -> Result<Vec<ProjectedConfig>, RecordError> {
        let mut winners = BTreeMap::<String, ProjectedConfig>::new();
        for (key, value) in self.workspace.list(CONFIG_PREFIX).await? {
            let key = String::from_utf8_lossy(&key).into_owned();
            let record: ConfigRevision = decode(&value)?;
            record.validate()?;
            let revision_id = record.id()?;
            if key != config_key(&record, &revision_id) {
                return Err(RecordError::KeyMismatch(key));
            }
            let bytes = self
                .workspace
                .get(config_blob_key(&revision_id))
                .await?
                .ok_or_else(|| RecordError::MissingContent(record.path.clone()))?;
            if u64::try_from(bytes.len()).ok() != Some(record.size)
                || blake3::hash(&bytes).to_hex().as_str() != record.blob_hash
            {
                return Err(RecordError::ContentMismatch(record.path));
            }
            let candidate = ProjectedConfig {
                revision_id,
                record,
                bytes,
            };
            let replace = winners.get(&candidate.record.path).is_none_or(|current| {
                (&candidate.record.hlc, &candidate.revision_id)
                    > (&current.record.hlc, &current.revision_id)
            });
            if replace {
                winners.insert(candidate.record.path.clone(), candidate);
            }
        }
        Ok(winners.into_values().collect())
    }

    pub async fn put_tombstone(&self, record: &Tombstone) -> Result<(), RecordError> {
        record.validate()?;
        self.workspace
            .put(tombstone_key(record), encode(record)?)
            .await?;
        Ok(())
    }

    pub async fn list_tombstones(&self) -> Result<Vec<Tombstone>, RecordError> {
        list_typed(self.workspace, TOMBSTONE_PREFIX, Tombstone::validate).await
    }

    pub async fn put_device(&self, record: &DeviceRecord) -> Result<(), RecordError> {
        record.validate()?;
        self.workspace
            .put(device_key(record), encode(record)?)
            .await?;
        Ok(())
    }

    pub async fn list_devices(&self) -> Result<Vec<DeviceRecord>, RecordError> {
        list_typed(self.workspace, DEVICE_PREFIX, DeviceRecord::validate).await
    }

    pub async fn put_descriptor(
        &self,
        descriptor: &WorkspaceDescriptor,
    ) -> Result<(), RecordError> {
        descriptor.validate()?;
        self.workspace
            .put(WORKSPACE_DESCRIPTOR_KEY, encode(descriptor)?)
            .await?;
        Ok(())
    }

    pub async fn descriptor(&self) -> Result<Option<WorkspaceDescriptor>, RecordError> {
        let Some(bytes) = self.workspace.get(WORKSPACE_DESCRIPTOR_KEY).await? else {
            return Ok(None);
        };
        let descriptor: WorkspaceDescriptor = decode(&bytes)?;
        descriptor.validate()?;
        Ok(Some(descriptor))
    }

    /// Resolve every note from Docs/Blobs while retaining invalid-record diagnostics.
    pub async fn snapshot(&self) -> Result<WorkspaceSnapshot, RecordError> {
        let mut groups = BTreeMap::<NoteId, NoteRecords>::new();
        let mut snapshot = WorkspaceSnapshot::default();

        for (raw_key, value) in self.workspace.list(NOTE_PREFIX).await? {
            let key = String::from_utf8_lossy(&raw_key).into_owned();
            let Some(note_id) = note_id_from_key(&key) else {
                snapshot.diagnostics.push(record_diagnostic(
                    &key,
                    "record key must be note/{note_id}/{revision|head}/{id}",
                ));
                continue;
            };
            let group = groups.entry(note_id.clone()).or_default();
            if let Err(error) = decode_record(&key, &value, Some(&note_id), group) {
                snapshot
                    .diagnostics
                    .push(record_diagnostic(&key, &error.to_string()));
            }
        }

        for (note_id, group) in groups {
            match resolve_group(&group) {
                Ok(Some(resolved)) => {
                    if let Some(revision) = &resolved.visible {
                        snapshot.notes.push(Note {
                            id: revision.note_id.clone(),
                            frontmatter: revision.frontmatter.clone(),
                            body: revision.body.clone(),
                            path: revision.materialized_path.clone(),
                        });
                    }
                    snapshot.resolved.push(resolved);
                }
                Ok(None) => {}
                Err(error) => snapshot.diagnostics.push(record_diagnostic(
                    &format!("{NOTE_PREFIX}{note_id}"),
                    &error.to_string(),
                )),
            }
        }
        snapshot
            .notes
            .sort_by(|left, right| left.path.cmp(&right.path));
        match self.list_assets().await {
            Ok(assets) => snapshot.assets = assets,
            Err(error) => snapshot
                .diagnostics
                .push(record_diagnostic(ASSET_PREFIX, &error.to_string())),
        }
        snapshot.configs = self.list_configs().await?;
        snapshot.tombstones = self.list_tombstones().await?;
        snapshot.devices = self.list_devices().await?;
        snapshot.descriptor = self.descriptor().await?;
        Ok(snapshot)
    }

    /// Replace the disposable `SQLite` index from authoritative resolved records.
    pub async fn rebuild_index(
        &self,
        index: &LocalIndex,
    ) -> Result<WorkspaceSnapshot, RecordError> {
        let snapshot = self.snapshot().await?;
        index.rebuild(&snapshot.notes, &snapshot.diagnostics)?;
        Ok(snapshot)
    }
}

#[derive(Default)]
struct NoteRecords {
    revisions: BTreeMap<RevisionId, NoteRevision>,
    heads: Vec<Head>,
}

fn revision_key(note_id: &NoteId, revision_id: &RevisionId) -> String {
    format!("{NOTE_PREFIX}{note_id}/revision/{revision_id}")
}

fn head_key(note_id: &NoteId, author_id: &ActorId) -> String {
    format!("{NOTE_PREFIX}{note_id}/head/{author_id}")
}

fn asset_key(asset_id: &AssetId) -> String {
    format!("{ASSET_PREFIX}{asset_id}")
}

fn asset_blob_key(asset_id: &AssetId) -> String {
    format!("{ASSET_BLOB_PREFIX}{asset_id}")
}

fn config_key(record: &ConfigRevision, revision_id: &RevisionId) -> String {
    format!("{CONFIG_PREFIX}{}/{revision_id}", record.path)
}

fn config_blob_key(revision_id: &RevisionId) -> String {
    format!("{CONFIG_BLOB_PREFIX}{revision_id}")
}

fn tombstone_key(record: &Tombstone) -> String {
    format!(
        "{TOMBSTONE_PREFIX}{}/{}",
        record.target_id, record.author_id
    )
}

fn device_key(record: &DeviceRecord) -> String {
    format!("{DEVICE_PREFIX}{}", record.endpoint_id)
}

fn note_id_from_key(key: &str) -> Option<NoteId> {
    let mut parts = key.split('/');
    let valid = parts.next() == Some("note")
        && parts.next().is_some_and(crate::id::is_valid)
        && matches!(parts.next(), Some("revision" | "head"))
        && parts.next().is_some_and(|part| !part.is_empty())
        && parts.next().is_none();
    valid.then(|| NoteId::new(key.split('/').nth(1).expect("validated note key")))
}

fn decode_record(
    key: &str,
    value: &[u8],
    expected_note: Option<&NoteId>,
    group: &mut NoteRecords,
) -> Result<(), RecordError> {
    let parts = key.split('/').collect::<Vec<_>>();
    if parts.len() != 4 || parts[0] != "note" || !crate::id::is_valid(parts[1]) {
        return Err(RecordError::InvalidKey(key.to_owned()));
    }
    let note_id = NoteId::new(parts[1]);
    if expected_note.is_some_and(|expected| expected != &note_id) {
        return Err(RecordError::KeyMismatch(key.to_owned()));
    }
    match parts[2] {
        "revision" => {
            let revision: NoteRevision = decode(value)?;
            revision.validate()?;
            let revision_id = revision.id()?;
            if revision.note_id != note_id || revision_id.as_str() != parts[3] {
                return Err(RecordError::KeyMismatch(key.to_owned()));
            }
            group.revisions.insert(revision_id, revision);
        }
        "head" => {
            let head: Head = decode(value)?;
            head.validate()?;
            if head.note_id != note_id || head.author_id.as_str() != parts[3] {
                return Err(RecordError::KeyMismatch(key.to_owned()));
            }
            group.heads.push(head);
        }
        _ => return Err(RecordError::InvalidKey(key.to_owned())),
    }
    Ok(())
}

fn resolve_group(group: &NoteRecords) -> Result<Option<ResolvedNote>, RecordError> {
    validate_revision_graph(&group.revisions)?;
    for head in &group.heads {
        if !group.revisions.contains_key(&head.revision_id) {
            return Err(RecordError::MissingRevision(head.revision_id.clone()));
        }
    }
    Ok(resolve_heads(&group.revisions, &group.heads))
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, RecordError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes)
        .map_err(|error| RecordError::Encoding(error.to_string()))?;
    Ok(bytes)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, RecordError> {
    ciborium::from_reader(bytes).map_err(|error| RecordError::Decoding(error.to_string()))
}

async fn list_typed<T>(
    workspace: &IrohWorkspace,
    prefix: &str,
    validate: fn(&T) -> Result<(), DomainError>,
) -> Result<Vec<T>, RecordError>
where
    T: DeserializeOwned + Ord,
{
    let mut records = Vec::new();
    for (_, bytes) in workspace.list(prefix).await? {
        let record: T = decode(&bytes)?;
        validate(&record)?;
        records.push(record);
    }
    records.sort();
    Ok(records)
}

fn record_diagnostic(path: &str, message: &str) -> Diagnostic {
    Diagnostic {
        path: path.to_owned(),
        code: "invalid-record".to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use crate::domain::{Frontmatter, FrontmatterValue};
    use crate::iroh_node::IrohNode;
    use crate::{Hlc, SchemaVersion};

    use super::*;

    async fn add_metadata_records(
        node: &IrohNode,
        workspace: &crate::iroh_node::IrohWorkspace,
        records: WorkspaceRecords<'_>,
        revision_id: RevisionId,
    ) -> anyhow::Result<()> {
        let actor = records.actor_id();
        let hlc = Hlc {
            physical_ms: 2,
            logical: 0,
            actor_id: actor.clone(),
        };
        records
            .put_config(
                "exo.scm",
                b"(workspace)".to_vec(),
                hlc.clone(),
                BTreeSet::new(),
            )
            .await?;
        records
            .put_tombstone(&Tombstone {
                schema: CURRENT_SCHEMA,
                target_id: "obsolete".to_owned(),
                author_id: actor.clone(),
                revision_id,
                hlc,
            })
            .await?;
        records
            .put_device(&DeviceRecord {
                schema: CURRENT_SCHEMA,
                endpoint_id: node.endpoint_id().to_string(),
                author_id: actor,
                label: "test device".to_owned(),
                capabilities: BTreeSet::from(["write".to_owned()]),
                last_seen_ms: Some(2),
                retired_at: None,
            })
            .await?;
        records
            .put_descriptor(&WorkspaceDescriptor {
                schema: CURRENT_SCHEMA,
                workspace_id: crate::WorkspaceId::new(workspace.id().to_string()),
                docs_ticket: workspace.share(false).await?,
                bootstrap_peers: vec![node.endpoint_id().to_string()],
                relay_mode: "default".to_owned(),
                encrypted_workspace_key: None,
                read_only: true,
            })
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn committed_records_resolve_and_rebuild_the_index() -> anyhow::Result<()> {
        let _guard = crate::iroh_node::IROH_TEST_LOCK.lock().await;
        let directory = tempfile::tempdir()?;
        let node = IrohNode::persistent(directory.path().join("iroh")).await?;
        let workspace = node.create_workspace().await?;
        let author = ActorId::new(workspace.author_id().to_string());
        let revision = NoteRevision {
            schema: SchemaVersion(1),
            note_id: NoteId::new("note002"),
            frontmatter: Frontmatter::from([
                (
                    "id".to_owned(),
                    FrontmatterValue::String("note002".to_owned()),
                ),
                (
                    "title".to_owned(),
                    FrontmatterValue::String("Stored".to_owned()),
                ),
            ]),
            body: "authoritative\n".to_owned(),
            materialized_path: "notes/stored.md".to_owned(),
            hlc: Hlc {
                physical_ms: 1,
                logical: 0,
                actor_id: author.clone(),
            },
            author_id: author,
            predecessors: BTreeSet::new(),
            deleted: false,
        };
        let records = WorkspaceRecords::new(&workspace);
        let revision_id = records.commit_revision(&revision).await?;
        add_metadata_records(&node, &workspace, records, revision_id.clone()).await?;
        let asset = records
            .put_asset(
                AssetId::new("image001"),
                "image/png",
                "assets/example.png",
                b"asset bytes".to_vec(),
            )
            .await?;
        assert_eq!(
            records
                .get_asset(&asset.id)
                .await?
                .expect("stored asset")
                .bytes,
            b"asset bytes"
        );
        assert_eq!(
            records
                .load_note(&revision.note_id)
                .await?
                .expect("resolved note")
                .winning_revision,
            revision_id
        );

        let index = LocalIndex::open(directory.path().join("index.sqlite"))?;
        let snapshot = records.rebuild_index(&index).await?;
        assert_eq!(snapshot.notes.len(), 1);
        assert_eq!(snapshot.assets.len(), 1);
        assert_eq!(snapshot.configs[0].bytes, b"(workspace)");
        assert_eq!(snapshot.tombstones.len(), 1);
        assert_eq!(snapshot.devices.len(), 1);
        assert_eq!(
            snapshot
                .descriptor
                .expect("workspace descriptor")
                .relay_mode,
            "default"
        );
        assert_eq!(index.all()?.first().expect("indexed note").title, "Stored");
        node.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn asset_record_and_blob_replicate_to_a_second_peer() -> anyhow::Result<()> {
        let _guard = crate::iroh_node::IROH_TEST_LOCK.lock().await;
        let first_dir = tempfile::tempdir()?;
        let second_dir = tempfile::tempdir()?;
        let first = IrohNode::persistent(first_dir.path()).await?;
        let workspace = first.create_workspace().await?;
        WorkspaceRecords::new(&workspace)
            .put_asset(
                AssetId::new("image001"),
                "image/png",
                "assets/example.png",
                b"replicated asset".to_vec(),
            )
            .await?;
        let ticket = workspace.share(true).await?;

        let second = IrohNode::persistent(second_dir.path()).await?;
        let imported = second.import_workspace(&ticket).await?;
        let records = WorkspaceRecords::new(&imported);
        let mut replicated = None;
        for _ in 0..100 {
            match records.get_asset(&AssetId::new("image001")).await {
                Ok(Some(asset)) => {
                    replicated = Some(asset);
                    break;
                }
                Ok(None) | Err(RecordError::MissingBlob(_)) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
        assert_eq!(
            replicated.expect("replicated asset").bytes,
            b"replicated asset"
        );
        second.shutdown().await?;
        first.shutdown().await?;
        Ok(())
    }

    fn isolated_revision(actor: ActorId, body: &str) -> NoteRevision {
        NoteRevision {
            schema: CURRENT_SCHEMA,
            note_id: NoteId::new("note002"),
            frontmatter: crate::domain::Frontmatter::from([(
                "id".to_owned(),
                crate::domain::FrontmatterValue::String("note002".to_owned()),
            )]),
            body: body.to_owned(),
            materialized_path: "notes/concurrent.md".to_owned(),
            hlc: Hlc {
                physical_ms: 100,
                logical: 0,
                actor_id: actor.clone(),
            },
            author_id: actor,
            predecessors: BTreeSet::new(),
            deleted: false,
        }
    }

    async fn wait_for_conflict(records: WorkspaceRecords<'_>) -> anyhow::Result<ResolvedNote> {
        for _ in 0..200 {
            match records.load_note(&NoteId::new("note002")).await {
                Ok(Some(resolved))
                    if resolved
                        .conflict
                        .as_ref()
                        .is_some_and(|conflict| conflict.concurrent_revisions.len() == 1) =>
                {
                    return Ok(resolved);
                }
                Ok(_) | Err(RecordError::Transport(_)) => {}
                Err(error) => return Err(error.into()),
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        anyhow::bail!("peer did not converge on both concurrent revisions")
    }

    #[tokio::test]
    async fn three_persisted_peers_converge_after_isolated_edits_and_restarts() -> anyhow::Result<()>
    {
        let _guard = crate::iroh_node::IROH_TEST_LOCK.lock().await;
        let a_dir = tempfile::tempdir()?;
        let b_dir = tempfile::tempdir()?;
        let c_dir = tempfile::tempdir()?;
        let a = IrohNode::persistent(a_dir.path()).await?;
        let workspace_a = a.create_workspace().await?;
        let workspace_id = workspace_a.id();
        let ticket = workspace_a.share(true).await?;
        let b = IrohNode::persistent(b_dir.path()).await?;
        b.import_workspace(&ticket).await?;
        let c = IrohNode::persistent(c_dir.path()).await?;
        c.import_workspace(&ticket).await?;
        c.shutdown().await?;
        b.shutdown().await?;
        a.shutdown().await?;
        drop((workspace_a, a, b, c));

        let b = IrohNode::persistent(b_dir.path()).await?;
        let workspace_b = b.open_workspace(workspace_id).await?.expect("workspace B");
        let records_b = WorkspaceRecords::new(&workspace_b);
        records_b
            .commit_revision(&isolated_revision(records_b.actor_id(), "offline B"))
            .await?;
        b.shutdown().await?;
        drop((workspace_b, b));

        let c = IrohNode::persistent(c_dir.path()).await?;
        let workspace_c = c.open_workspace(workspace_id).await?.expect("workspace C");
        let records_c = WorkspaceRecords::new(&workspace_c);
        records_c
            .commit_revision(&isolated_revision(records_c.actor_id(), "offline C"))
            .await?;
        c.shutdown().await?;
        drop((workspace_c, c));

        let a = IrohNode::persistent(a_dir.path()).await?;
        let workspace_a = a.open_workspace(workspace_id).await?.expect("workspace A");
        let reconnect_ticket = workspace_a.share(true).await?;
        let b = IrohNode::persistent(b_dir.path()).await?;
        let workspace_b = b.open_workspace(workspace_id).await?.expect("workspace B");
        workspace_b.start_sync(&reconnect_ticket).await?;
        let c = IrohNode::persistent(c_dir.path()).await?;
        let workspace_c = c.open_workspace(workspace_id).await?.expect("workspace C");
        workspace_c.start_sync(&reconnect_ticket).await?;

        let resolved_a = wait_for_conflict(WorkspaceRecords::new(&workspace_a)).await?;
        let resolved_b = wait_for_conflict(WorkspaceRecords::new(&workspace_b)).await?;
        let resolved_c = wait_for_conflict(WorkspaceRecords::new(&workspace_c)).await?;
        assert_eq!(resolved_a, resolved_b);
        assert_eq!(resolved_b, resolved_c);
        c.shutdown().await?;
        b.shutdown().await?;
        a.shutdown().await?;
        Ok(())
    }
}
