//! Typed exokephalos records stored directly in an Automerge workspace.

use std::collections::BTreeMap;

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::central_replica::CentralReplica;
use crate::local_index::{IndexError, LocalIndex};
use crate::projection::{Diagnostic, ProjectedAsset};
use crate::record_workspace::RecordWorkspace;
use crate::{
    ActorId, AssetId, AssetRecord, CURRENT_SCHEMA, ConfigRevision, DomainError, Head, Hlc, Note,
    NoteId, NoteRevision, ResolvedNote, RevisionGraphError, RevisionId, Tombstone, resolve_heads,
    validate_revision_graph,
};

const NOTE_PREFIX: &str = "note/";
const ASSET_PREFIX: &str = "asset/";
const ASSET_BLOB_PREFIX: &str = "asset-blob/";
const CONFIG_PREFIX: &str = "config/";
const CONFIG_BLOB_PREFIX: &str = "config-blob/";
const TOMBSTONE_PREFIX: &str = "tombstone/";

#[derive(Debug, Error)]
pub enum RecordError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Graph(#[from] RevisionGraphError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error("record storage failed: {0}")]
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
    #[error("record author does not match its key: {0}")]
    AuthorMismatch(String),
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
    pub diagnostics: Vec<Diagnostic>,
}

/// The authoritative revision and per-author head repository for a workspace.
#[derive(Debug)]
pub struct WorkspaceRecords<'a, W: RecordWorkspace = CentralReplica> {
    workspace: &'a W,
}

impl<W: RecordWorkspace> Copy for WorkspaceRecords<'_, W> {}

impl<W: RecordWorkspace> Clone for WorkspaceRecords<'_, W> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, W: RecordWorkspace> WorkspaceRecords<'a, W> {
    #[must_use]
    pub const fn new(workspace: &'a W) -> Self {
        Self { workspace }
    }

    #[must_use]
    pub fn actor_id(&self) -> ActorId {
        self.workspace.record_actor_id()
    }

    pub async fn put_revision(&self, revision: &NoteRevision) -> Result<RevisionId, RecordError> {
        self.ensure_local_author(&revision.author_id, &revision.hlc)?;
        let revision_id = revision.id()?;
        self.workspace
            .put_record(
                revision_key(&revision.note_id, &revision_id),
                revision.canonical_bytes()?,
            )
            .await?;
        Ok(revision_id)
    }

    pub async fn set_head(&self, head: &Head) -> Result<(), RecordError> {
        head.validate()?;
        if head.author_id != self.actor_id() {
            return Err(RecordError::AuthorMismatch(head_key(
                &head.note_id,
                &head.author_id,
            )));
        }
        if self
            .workspace
            .get_record(revision_key(&head.note_id, &head.revision_id))
            .await?
            .is_none()
        {
            return Err(RecordError::MissingRevision(head.revision_id.clone()));
        }
        self.workspace
            .put_record(head_key(&head.note_id, &head.author_id), encode(head)?)
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
        for entry in self.workspace.list_authored_records(prefix).await? {
            let key = String::from_utf8(entry.key).map_err(|error| {
                RecordError::InvalidKey(String::from_utf8_lossy(error.as_bytes()).into_owned())
            })?;
            decode_record(
                &key,
                &entry.value,
                &ActorId::new(entry.author),
                Some(note_id),
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
        for entry in self.workspace.list_authored_records(prefix).await? {
            let key = String::from_utf8(entry.key).map_err(|error| {
                RecordError::InvalidKey(String::from_utf8_lossy(error.as_bytes()).into_owned())
            })?;
            decode_record(
                &key,
                &entry.value,
                &ActorId::new(entry.author),
                Some(note_id),
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
        let mut groups = BTreeMap::<NoteId, NoteRecords>::new();
        for entry in self.workspace.list_authored_records(NOTE_PREFIX).await? {
            let key = String::from_utf8_lossy(&entry.key).into_owned();
            let Some(note_id) = note_id_from_key(&key) else {
                continue;
            };
            decode_record(
                &key,
                &entry.value,
                &ActorId::new(entry.author),
                Some(&note_id),
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
            .get_authored_record(revision_key(note_id, revision_id))
            .await?
        else {
            return Ok(None);
        };
        let revision: NoteRevision = decode(&entry.value)?;
        revision.validate()?;
        if revision.note_id != *note_id || revision.id()? != *revision_id {
            return Err(RecordError::KeyMismatch(revision_key(note_id, revision_id)));
        }
        verify_author(
            &revision_key(note_id, revision_id),
            &revision.author_id,
            &ActorId::new(entry.author),
        )?;
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
        let blob_hash = self
            .workspace
            .put_blob_record(asset_blob_key(&id), bytes)
            .await?;
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
            .put_record(asset_key(&record.id), encode(&record)?)
            .await?;
        Ok(record)
    }

    pub async fn get_asset(&self, id: &AssetId) -> Result<Option<ProjectedAsset>, RecordError> {
        let Some(record_bytes) = self.workspace.get_record(asset_key(id)).await? else {
            return Ok(None);
        };
        let record: AssetRecord = decode(&record_bytes)?;
        record.validate()?;
        if record.id != *id {
            return Err(RecordError::KeyMismatch(asset_key(id)));
        }
        let bytes = self
            .workspace
            .get_record(asset_blob_key(id))
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
        for (key, _) in self.workspace.list_records(ASSET_PREFIX).await? {
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
        self.ensure_local_author(&hlc.actor_id, &hlc)?;
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
            .put_blob_record(config_blob_key(&revision_id), bytes.clone())
            .await?;
        if stored_hash != record.blob_hash {
            return Err(RecordError::ContentMismatch(record.path));
        }
        self.workspace
            .put_record(config_key(&record, &revision_id), encode(&record)?)
            .await?;
        Ok(ProjectedConfig {
            revision_id,
            record,
            bytes,
        })
    }

    pub async fn list_configs(&self) -> Result<Vec<ProjectedConfig>, RecordError> {
        let mut winners = BTreeMap::<String, ProjectedConfig>::new();
        for entry in self.workspace.list_authored_records(CONFIG_PREFIX).await? {
            let key = String::from_utf8_lossy(&entry.key).into_owned();
            let record: ConfigRevision = decode(&entry.value)?;
            record.validate()?;
            let revision_id = record.id()?;
            if key != config_key(&record, &revision_id) {
                return Err(RecordError::KeyMismatch(key));
            }
            verify_author(&key, &record.author_id, &ActorId::new(entry.author))?;
            let bytes = self
                .workspace
                .get_record(config_blob_key(&revision_id))
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
        self.ensure_local_author(&record.author_id, &record.hlc)?;
        self.workspace
            .put_record(tombstone_key(record), encode(record)?)
            .await?;
        Ok(())
    }

    pub async fn list_tombstones(&self) -> Result<Vec<Tombstone>, RecordError> {
        let mut records = Vec::new();
        for entry in self
            .workspace
            .list_authored_records(TOMBSTONE_PREFIX)
            .await?
        {
            let key = String::from_utf8_lossy(&entry.key).into_owned();
            let record: Tombstone = decode(&entry.value)?;
            record.validate()?;
            verify_author(&key, &record.author_id, &ActorId::new(entry.author))?;
            records.push(record);
        }
        records.sort();
        Ok(records)
    }

    fn ensure_local_author(self, author: &ActorId, _hlc: &Hlc) -> Result<(), RecordError> {
        verify_author("local write", author, &self.actor_id())
    }

    /// Resolve every note from Automerge while retaining invalid-record diagnostics.
    pub async fn snapshot(&self) -> Result<WorkspaceSnapshot, RecordError> {
        let mut groups = BTreeMap::<NoteId, NoteRecords>::new();
        let mut snapshot = WorkspaceSnapshot::default();

        for entry in self.workspace.list_authored_records(NOTE_PREFIX).await? {
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

fn verify_author(key: &str, claimed: &ActorId, stored_author: &ActorId) -> Result<(), RecordError> {
    if claimed == stored_author {
        Ok(())
    } else {
        Err(RecordError::AuthorMismatch(key.to_owned()))
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
    stored_author: &ActorId,
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
            verify_author(key, &revision.author_id, stored_author)?;
            group.revisions.insert(revision_id, revision);
        }
        "head" => {
            let head: Head = decode(value)?;
            head.validate()?;
            if head.note_id != note_id || head.author_id.as_str() != parts[3] {
                return Err(RecordError::KeyMismatch(key.to_owned()));
            }
            verify_author(key, &head.author_id, stored_author)?;
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

fn record_diagnostic(path: &str, message: &str) -> Diagnostic {
    Diagnostic {
        path: path.to_owned(),
        code: "invalid-record".to_owned(),
        message: message.to_owned(),
    }
}
