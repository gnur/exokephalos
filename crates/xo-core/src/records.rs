//! Typed exokephalos records stored directly in an Automerge workspace.

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
    #[error("record signer does not match its claimed author: {0}")]
    SignerMismatch(String),
    #[error("write by retired author is after its cutoff: {0}")]
    RetiredWrite(ActorId),
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
        self.ensure_local_author(&revision.author_id, &revision.hlc)
            .await?;
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
        if head.author_id != self.actor_id() {
            return Err(RecordError::SignerMismatch(head_key(
                &head.note_id,
                &head.author_id,
            )));
        }
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
        let cutoffs = self.retirement_cutoffs().await?;
        for entry in self.workspace.list_signed(prefix).await? {
            let key = String::from_utf8(entry.key).map_err(|error| {
                RecordError::InvalidKey(String::from_utf8_lossy(error.as_bytes()).into_owned())
            })?;
            decode_record(
                &key,
                &entry.value,
                &ActorId::new(entry.author),
                Some(note_id),
                &cutoffs,
                &mut group,
            )?;
        }
        resolve_group(&group)
    }

    /// Return accepted immutable history in deterministic HLC/revision order.
    pub async fn revision_history(
        &self,
        note_id: &NoteId,
    ) -> Result<Vec<(RevisionId, NoteRevision)>, RecordError> {
        let prefix = format!("{NOTE_PREFIX}{note_id}/");
        let mut group = NoteRecords::default();
        let cutoffs = self.retirement_cutoffs().await?;
        for entry in self.workspace.list_signed(prefix).await? {
            let key = String::from_utf8(entry.key).map_err(|error| {
                RecordError::InvalidKey(String::from_utf8_lossy(error.as_bytes()).into_owned())
            })?;
            decode_record(
                &key,
                &entry.value,
                &ActorId::new(entry.author),
                Some(note_id),
                &cutoffs,
                &mut group,
            )?;
        }
        let mut history = group.revisions.into_iter().collect::<Vec<_>>();
        history.sort_by(|(left_id, left), (right_id, right)| {
            left.hlc.cmp(&right.hlc).then_with(|| left_id.cmp(right_id))
        });
        Ok(history)
    }

    /// Return the latest content of currently deleted notes so frontends can restore them.
    pub async fn deleted_notes(&self) -> Result<Vec<Note>, RecordError> {
        let cutoffs = self.retirement_cutoffs().await?;
        let mut groups = BTreeMap::<NoteId, NoteRecords>::new();
        for entry in self.workspace.list_signed(NOTE_PREFIX).await? {
            let key = String::from_utf8_lossy(&entry.key).into_owned();
            let Some(note_id) = note_id_from_key(&key) else {
                continue;
            };
            decode_record(
                &key,
                &entry.value,
                &ActorId::new(entry.author),
                Some(&note_id),
                &cutoffs,
                groups.entry(note_id.clone()).or_default(),
            )?;
        }
        let mut notes = Vec::new();
        for group in groups.values() {
            let Some(resolved) = resolve_group(group)? else {
                continue;
            };
            if resolved.visible.is_some() {
                continue;
            }
            if let Some(revision) = group.revisions.get(&resolved.winning_revision) {
                notes.push(Note {
                    id: revision.note_id.clone(),
                    frontmatter: revision.frontmatter.clone(),
                    body: revision.body.clone(),
                    path: revision.materialized_path.clone(),
                });
            }
        }
        notes.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(notes)
    }

    pub async fn get_revision(
        &self,
        note_id: &NoteId,
        revision_id: &RevisionId,
    ) -> Result<Option<NoteRevision>, RecordError> {
        let Some(entry) = self
            .workspace
            .get_signed(revision_key(note_id, revision_id))
            .await?
        else {
            return Ok(None);
        };
        let revision: NoteRevision = decode(&entry.value)?;
        revision.validate()?;
        if revision.note_id != *note_id || revision.id()? != *revision_id {
            return Err(RecordError::KeyMismatch(revision_key(note_id, revision_id)));
        }
        verify_signer(
            &revision_key(note_id, revision_id),
            &revision.author_id,
            &ActorId::new(entry.author),
        )?;
        if !self
            .retirement_cutoffs()
            .await?
            .allows(&revision.author_id, &revision.hlc)
        {
            return Ok(None);
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
        self.ensure_local_author(&hlc.actor_id, &hlc).await?;
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
        let cutoffs = self.retirement_cutoffs().await?;
        for entry in self.workspace.list_signed(CONFIG_PREFIX).await? {
            let key = String::from_utf8_lossy(&entry.key).into_owned();
            let record: ConfigRevision = decode(&entry.value)?;
            record.validate()?;
            let revision_id = record.id()?;
            if key != config_key(&record, &revision_id) {
                return Err(RecordError::KeyMismatch(key));
            }
            verify_signer(&key, &record.author_id, &ActorId::new(entry.author))?;
            if !cutoffs.allows(&record.author_id, &record.hlc) {
                continue;
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
        self.ensure_local_author(&record.author_id, &record.hlc)
            .await?;
        self.workspace
            .put(tombstone_key(record), encode(record)?)
            .await?;
        Ok(())
    }

    pub async fn list_tombstones(&self) -> Result<Vec<Tombstone>, RecordError> {
        let cutoffs = self.retirement_cutoffs().await?;
        let mut records = Vec::new();
        for entry in self.workspace.list_signed(TOMBSTONE_PREFIX).await? {
            let key = String::from_utf8_lossy(&entry.key).into_owned();
            let record: Tombstone = decode(&entry.value)?;
            record.validate()?;
            verify_signer(&key, &record.author_id, &ActorId::new(entry.author))?;
            if cutoffs.allows(&record.author_id, &record.hlc) {
                records.push(record);
            }
        }
        records.sort();
        Ok(records)
    }

    pub async fn put_device(&self, record: &DeviceRecord) -> Result<(), RecordError> {
        record.validate()?;
        let expected_signer = record
            .retired_at
            .as_ref()
            .map_or(&record.author_id, |cutoff| &cutoff.actor_id);
        verify_signer(&device_key(record), expected_signer, &self.actor_id())?;
        self.workspace
            .put(device_key(record), encode(record)?)
            .await?;
        Ok(())
    }

    pub async fn list_devices(&self) -> Result<Vec<DeviceRecord>, RecordError> {
        let mut records = Vec::new();
        for entry in self.workspace.list_signed(DEVICE_PREFIX).await? {
            let key = String::from_utf8_lossy(&entry.key).into_owned();
            let record: DeviceRecord = decode(&entry.value)?;
            record.validate()?;
            if key != device_key(&record) {
                return Err(RecordError::KeyMismatch(key));
            }
            let expected_signer = record
                .retired_at
                .as_ref()
                .map_or(&record.author_id, |cutoff| &cutoff.actor_id);
            verify_signer(&key, expected_signer, &ActorId::new(entry.author))?;
            records.push(record);
        }
        records.sort();
        Ok(records)
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

    async fn retirement_cutoffs(&self) -> Result<RetirementCutoffs, RecordError> {
        Ok(RetirementCutoffs::from_devices(&self.list_devices().await?))
    }

    async fn ensure_local_author(&self, author: &ActorId, hlc: &Hlc) -> Result<(), RecordError> {
        verify_signer("local write", author, &self.actor_id())?;
        if self.retirement_cutoffs().await?.allows(author, hlc) {
            Ok(())
        } else {
            Err(RecordError::RetiredWrite(author.clone()))
        }
    }

    /// Resolve every note from Automerge while retaining invalid-record diagnostics.
    pub async fn snapshot(&self) -> Result<WorkspaceSnapshot, RecordError> {
        let mut groups = BTreeMap::<NoteId, NoteRecords>::new();
        let mut snapshot = WorkspaceSnapshot {
            devices: self.list_devices().await?,
            ..WorkspaceSnapshot::default()
        };
        let cutoffs = RetirementCutoffs::from_devices(&snapshot.devices);

        for entry in self.workspace.list_signed(NOTE_PREFIX).await? {
            let key = String::from_utf8_lossy(&entry.key).into_owned();
            let Some(note_id) = note_id_from_key(&key) else {
                snapshot.diagnostics.push(record_diagnostic(
                    &key,
                    "record key must be note/{note_id}/{revision|head}/{id}",
                ));
                continue;
            };
            let group = groups.entry(note_id.clone()).or_default();
            if let Err(error) = decode_record(
                &key,
                &entry.value,
                &ActorId::new(entry.author),
                Some(&note_id),
                &cutoffs,
                group,
            ) {
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
    retired_revisions: BTreeMap<RevisionId, NoteRevision>,
}

#[derive(Default)]
struct RetirementCutoffs(BTreeMap<ActorId, Hlc>);

impl RetirementCutoffs {
    fn from_devices(devices: &[DeviceRecord]) -> Self {
        let mut cutoffs = BTreeMap::<ActorId, Hlc>::new();
        for device in devices {
            let Some(cutoff) = &device.retired_at else {
                continue;
            };
            cutoffs
                .entry(device.author_id.clone())
                .and_modify(|current| {
                    if event_time(cutoff) < event_time(current) {
                        *current = cutoff.clone();
                    }
                })
                .or_insert_with(|| cutoff.clone());
        }
        Self(cutoffs)
    }

    fn allows(&self, author: &ActorId, hlc: &Hlc) -> bool {
        self.0
            .get(author)
            .is_none_or(|cutoff| event_time(hlc) <= event_time(cutoff))
    }
}

fn event_time(hlc: &Hlc) -> (u64, u32) {
    (hlc.physical_ms, hlc.logical)
}

fn verify_signer(key: &str, claimed: &ActorId, signer: &ActorId) -> Result<(), RecordError> {
    if claimed == signer {
        Ok(())
    } else {
        Err(RecordError::SignerMismatch(key.to_owned()))
    }
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
    signer: &ActorId,
    expected_note: Option<&NoteId>,
    cutoffs: &RetirementCutoffs,
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
            verify_signer(key, &revision.author_id, signer)?;
            if cutoffs.allows(&revision.author_id, &revision.hlc) {
                group.revisions.insert(revision_id, revision);
            } else {
                group.retired_revisions.insert(revision_id, revision);
            }
        }
        "head" => {
            let head: Head = decode(value)?;
            head.validate()?;
            if head.note_id != note_id || head.author_id.as_str() != parts[3] {
                return Err(RecordError::KeyMismatch(key.to_owned()));
            }
            verify_signer(key, &head.author_id, signer)?;
            group.heads.push(head);
        }
        _ => return Err(RecordError::InvalidKey(key.to_owned())),
    }
    Ok(())
}

fn resolve_group(group: &NoteRecords) -> Result<Option<ResolvedNote>, RecordError> {
    validate_revision_graph(&group.revisions)?;
    let mut heads = Vec::new();
    for head in &group.heads {
        if group.retired_revisions.contains_key(&head.revision_id) {
            let mut pending = vec![head.revision_id.clone()];
            let mut visited = std::collections::BTreeSet::new();
            while let Some(revision_id) = pending.pop() {
                if !visited.insert(revision_id.clone()) {
                    continue;
                }
                if group.revisions.contains_key(&revision_id) {
                    heads.push(Head {
                        note_id: head.note_id.clone(),
                        author_id: head.author_id.clone(),
                        revision_id,
                    });
                } else if let Some(revision) = group.retired_revisions.get(&revision_id) {
                    pending.extend(revision.predecessors.iter().cloned());
                }
            }
        } else {
            heads.push(head.clone());
        }
    }
    heads.sort_by(|left, right| {
        (&left.author_id, &left.revision_id).cmp(&(&right.author_id, &right.revision_id))
    });
    heads.dedup();
    for head in &heads {
        if !group.revisions.contains_key(&head.revision_id) {
            return Err(RecordError::MissingRevision(head.revision_id.clone()));
        }
    }
    Ok(resolve_heads(&group.revisions, &heads))
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
                "xo.scm",
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
                invitation: workspace.share(true).await?,
                bootstrap_peers: vec![node.endpoint_id().to_string()],
                relay_mode: "default".to_owned(),
                encrypted_workspace_key: None,
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
        let first =
            IrohNode::persistent_with_peer(first_dir.path(), crate::PeerId::parse("asset-first")?)
                .await?;
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

        let second = IrohNode::persistent_with_peer(
            second_dir.path(),
            crate::PeerId::parse("asset-second")?,
        )
        .await?;
        assert!(second.import_workspace(&ticket).await.is_err());
        let request = workspace.pending_requests().await.remove(0);
        workspace.approve_peer(&request.public_key).await?;
        let imported = second.import_workspace(&ticket).await?;
        let records = WorkspaceRecords::new(&imported);
        let mut replicated = None;
        for _ in 0..300 {
            match records.get_asset(&AssetId::new("image001")).await {
                Ok(Some(asset)) => {
                    replicated = Some(asset);
                    break;
                }
                Ok(None) | Err(RecordError::MissingBlob(_) | RecordError::Transport(_)) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
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

    async fn wait_for_device(
        records: WorkspaceRecords<'_>,
        author: &ActorId,
    ) -> anyhow::Result<DeviceRecord> {
        for _ in 0..200 {
            match records.list_devices().await {
                Ok(devices) => {
                    if let Some(device) = devices
                        .into_iter()
                        .find(|device| &device.author_id == author)
                    {
                        return Ok(device);
                    }
                }
                Err(RecordError::Transport(_)) => {}
                Err(error) => return Err(error.into()),
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        anyhow::bail!("device did not replicate")
    }

    async fn wait_for_retained_revision(
        records: WorkspaceRecords<'_>,
        revision_id: &RevisionId,
    ) -> anyhow::Result<ResolvedNote> {
        for _ in 0..200 {
            let retired = match records.list_devices().await {
                Ok(devices) => devices.iter().any(|device| device.retired_at.is_some()),
                Err(RecordError::Transport(_)) => false,
                Err(error) => return Err(error.into()),
            };
            if retired {
                match records.load_note(&NoteId::new("note002")).await {
                    Ok(Some(resolved)) if resolved.winning_revision == *revision_id => {
                        return Ok(resolved);
                    }
                    Ok(_) | Err(RecordError::Transport(_)) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        anyhow::bail!("retirement cutoff did not converge")
    }

    #[tokio::test]
    async fn signed_retirement_ignores_later_writes_and_retains_history() -> anyhow::Result<()> {
        let _guard = crate::iroh_node::IROH_TEST_LOCK.lock().await;
        let directory = tempfile::tempdir()?;
        let a_dir = directory.path().join("a");
        let b_dir = directory.path().join("b");
        let a = IrohNode::persistent_with_peer(&a_dir, crate::PeerId::parse("retire-a")?).await?;
        let workspace_a = a.create_workspace().await?;
        let workspace_id = workspace_a.id();
        let ticket = workspace_a.share(true).await?;
        let b = IrohNode::persistent_with_peer(&b_dir, crate::PeerId::parse("retire-b")?).await?;
        assert!(b.import_workspace(&ticket).await.is_err());
        let request = workspace_a.pending_requests().await.remove(0);
        workspace_a.approve_peer(&request.public_key).await?;
        let workspace_b = b.import_workspace(&ticket).await?;
        let records_b = WorkspaceRecords::new(&workspace_b);
        let author_b = records_b.actor_id();
        records_b
            .put_device(&DeviceRecord {
                schema: CURRENT_SCHEMA,
                endpoint_id: b.endpoint_id().to_string(),
                author_id: author_b.clone(),
                label: "device B".to_owned(),
                capabilities: BTreeSet::from(["write".to_owned()]),
                last_seen_ms: Some(100),
                retired_at: None,
            })
            .await?;
        let before = isolated_revision_at(author_b.clone(), "before retirement", 100, None);
        let before_id = records_b.commit_revision(&before).await?;

        let records_a = WorkspaceRecords::new(&workspace_a);
        let device_b = wait_for_device(records_a, &author_b).await?;
        b.shutdown().await?;
        a.shutdown().await?;
        drop((workspace_a, workspace_b, a, b));

        let a = IrohNode::persistent_with_peer(&a_dir, crate::PeerId::parse("retire-a")?).await?;
        let workspace_a = a.open_workspace(&workspace_id).await?.expect("workspace A");
        let records_a = WorkspaceRecords::new(&workspace_a);
        let cutoff = Hlc {
            physical_ms: 200,
            logical: 0,
            actor_id: records_a.actor_id(),
        };
        records_a
            .put_device(&DeviceRecord {
                retired_at: Some(cutoff),
                ..device_b
            })
            .await?;
        a.shutdown().await?;
        drop((workspace_a, a));

        // B has not received the retirement and can still create a signed offline write.
        let b = IrohNode::persistent_with_peer(&b_dir, crate::PeerId::parse("retire-b")?).await?;
        let workspace_b = b.open_workspace(&workspace_id).await?.expect("workspace B");
        let after =
            isolated_revision_at(author_b, "after retirement", 300, Some(before_id.clone()));
        let after_id = WorkspaceRecords::new(&workspace_b)
            .commit_revision(&after)
            .await?;
        b.shutdown().await?;
        drop((workspace_b, b));

        let a = IrohNode::persistent_with_peer(&a_dir, crate::PeerId::parse("retire-a")?).await?;
        let workspace_a = a.open_workspace(&workspace_id).await?.expect("workspace A");
        let reconnect = workspace_a.share(true).await?;
        let b = IrohNode::persistent_with_peer(&b_dir, crate::PeerId::parse("retire-b")?).await?;
        let workspace_b = b.open_workspace(&workspace_id).await?.expect("workspace B");
        workspace_b.start_sync(&reconnect).await?;
        let records_a = WorkspaceRecords::new(&workspace_a);
        let resolved = wait_for_retained_revision(records_a, &before_id).await?;
        assert_eq!(
            resolved.visible.expect("historical revision").body,
            "before retirement"
        );
        assert!(
            records_a
                .get_revision(&NoteId::new("note002"), &after_id)
                .await?
                .is_none()
        );
        b.shutdown().await?;
        a.shutdown().await?;
        Ok(())
    }

    fn isolated_revision(actor: ActorId, body: &str) -> NoteRevision {
        isolated_revision_at(actor, body, 100, None)
    }

    fn isolated_revision_at(
        actor: ActorId,
        body: &str,
        physical_ms: u64,
        predecessor: Option<RevisionId>,
    ) -> NoteRevision {
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
                physical_ms,
                logical: 0,
                actor_id: actor.clone(),
            },
            author_id: actor,
            predecessors: predecessor.into_iter().collect(),
            deleted: false,
        }
    }

    async fn wait_for_conflict(records: WorkspaceRecords<'_>) -> anyhow::Result<ResolvedNote> {
        let mut last_state = String::from("no observation");
        for _ in 0..900 {
            match records.load_note(&NoteId::new("note002")).await {
                Ok(Some(resolved))
                    if resolved
                        .conflict
                        .as_ref()
                        .is_some_and(|conflict| conflict.concurrent_revisions.len() == 1) =>
                {
                    return Ok(resolved);
                }
                Ok(Some(resolved)) => {
                    last_state = format!(
                        "visible revision {}, conflict={}",
                        resolved.winning_revision,
                        resolved.conflict.is_some()
                    );
                }
                Ok(None) => last_state = "note not available".to_owned(),
                Err(RecordError::Transport(error)) => {
                    last_state = format!("transport error: {error}");
                }
                Err(RecordError::MissingRevision(revision)) => {
                    last_state = format!("missing revision: {revision}");
                }
                Err(error) => return Err(error.into()),
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        anyhow::bail!("peer did not converge on both concurrent revisions after 90s ({last_state})")
    }

    #[tokio::test]
    #[ignore = "release-only extensive 1000-item workspace test"]
    async fn extensive_thousand_item_workspace_rebuilds_and_resolves() -> anyhow::Result<()> {
        let _guard = crate::iroh_node::IROH_TEST_LOCK.lock().await;
        let directory = tempfile::tempdir()?;
        let node = IrohNode::persistent(directory.path()).await?;
        let workspace = node.create_workspace().await?;
        let records = WorkspaceRecords::new(&workspace);
        for index in 0..1_000_u64 {
            let id = NoteId::new(format!("{:a>7}", crate::id::encode_base32(index + 1)));
            records
                .commit_revision(&NoteRevision {
                    schema: CURRENT_SCHEMA,
                    note_id: id.clone(),
                    frontmatter: Frontmatter::from([
                        ("id".to_owned(), FrontmatterValue::String(id.to_string())),
                        (
                            "title".to_owned(),
                            FrontmatterValue::String(format!("Knowledge item {index}")),
                        ),
                    ]),
                    body: format!("Body for knowledge item {index}"),
                    materialized_path: format!("bulk/{index}.md"),
                    hlc: Hlc {
                        physical_ms: 1_000_000 + index,
                        logical: 0,
                        actor_id: records.actor_id(),
                    },
                    author_id: records.actor_id(),
                    predecessors: BTreeSet::new(),
                    deleted: false,
                })
                .await?;
        }
        let snapshot = records.snapshot().await?;
        assert_eq!(snapshot.notes.len(), 1_000);
        assert_eq!(snapshot.resolved.len(), 1_000);
        assert!(snapshot.diagnostics.is_empty());
        node.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn three_persisted_peers_converge_after_isolated_edits_and_restarts() -> anyhow::Result<()>
    {
        let _guard = crate::iroh_node::IROH_TEST_LOCK.lock().await;
        let (relay_map, _relay_url, _relay_server) = iroh::test_utils::run_relay_server().await?;
        let a_dir = tempfile::tempdir()?;
        let b_dir = tempfile::tempdir()?;
        let c_dir = tempfile::tempdir()?;
        let a = IrohNode::persistent_with_relay_map(a_dir.path(), relay_map.clone()).await?;
        let workspace_a = a.create_workspace().await?;
        let workspace_id = workspace_a.id();
        let ticket = workspace_a.share(true).await?;
        let b = IrohNode::persistent_with_relay_map(b_dir.path(), relay_map.clone()).await?;
        assert!(b.import_workspace(&ticket).await.is_err());
        let request = workspace_a.pending_requests().await.remove(0);
        workspace_a.approve_peer(&request.public_key).await?;
        b.import_workspace(&ticket).await?;
        let c = IrohNode::persistent_with_relay_map(c_dir.path(), relay_map.clone()).await?;
        assert!(c.import_workspace(&ticket).await.is_err());
        let request = workspace_a.pending_requests().await.remove(0);
        workspace_a.approve_peer(&request.public_key).await?;
        c.import_workspace(&ticket).await?;
        c.shutdown().await?;
        b.shutdown().await?;
        a.shutdown().await?;
        drop((workspace_a, a, b, c));

        let b = IrohNode::persistent_with_relay_map(b_dir.path(), relay_map.clone()).await?;
        let workspace_b = b.open_workspace(&workspace_id).await?.expect("workspace B");
        let records_b = WorkspaceRecords::new(&workspace_b);
        records_b
            .commit_revision(&isolated_revision(records_b.actor_id(), "offline B"))
            .await?;
        b.shutdown().await?;
        drop((workspace_b, b));

        let c = IrohNode::persistent_with_relay_map(c_dir.path(), relay_map.clone()).await?;
        let workspace_c = c.open_workspace(&workspace_id).await?.expect("workspace C");
        let records_c = WorkspaceRecords::new(&workspace_c);
        records_c
            .commit_revision(&isolated_revision(records_c.actor_id(), "offline C"))
            .await?;
        c.shutdown().await?;
        drop((workspace_c, c));

        let a = IrohNode::persistent_with_relay_map(a_dir.path(), relay_map.clone()).await?;
        let workspace_a = a.open_workspace(&workspace_id).await?.expect("workspace A");
        let b = IrohNode::persistent_with_relay_map(b_dir.path(), relay_map.clone()).await?;
        let workspace_b = b.open_workspace(&workspace_id).await?.expect("workspace B");
        let c = IrohNode::persistent_with_relay_map(c_dir.path(), relay_map.clone()).await?;
        let workspace_c = c.open_workspace(&workspace_id).await?.expect("workspace C");

        let ticket_a = workspace_a.share(true).await?;
        let ticket_b = workspace_b.share(true).await?;
        let ticket_c = workspace_c.share(true).await?;
        // Serialize sync starts per workspace. Iroh Docs can coalesce concurrent
        // starts, which otherwise lets one readiness event race the next request.
        for _ in 0..2 {
            workspace_b.sync_and_wait(&ticket_a).await?;
            workspace_c.sync_and_wait(&ticket_a).await?;
            workspace_a.sync_and_wait(&ticket_b).await?;
            workspace_c.sync_and_wait(&ticket_b).await?;
            workspace_a.sync_and_wait(&ticket_c).await?;
            workspace_b.sync_and_wait(&ticket_c).await?;
        }

        let (resolved_a, resolved_b, resolved_c) = tokio::try_join!(
            wait_for_conflict(WorkspaceRecords::new(&workspace_a)),
            wait_for_conflict(WorkspaceRecords::new(&workspace_b)),
            wait_for_conflict(WorkspaceRecords::new(&workspace_c)),
        )?;
        assert_eq!(resolved_a, resolved_b);
        assert_eq!(resolved_b, resolved_c);
        c.shutdown().await?;
        b.shutdown().await?;
        a.shutdown().await?;
        Ok(())
    }
}
