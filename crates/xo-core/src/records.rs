//! Typed exokephalos records stored in an Iroh Docs workspace.

use std::collections::BTreeMap;

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::iroh_node::IrohWorkspace;
use crate::local_index::{IndexError, LocalIndex};
use crate::projection::Diagnostic;
use crate::{
    ActorId, DomainError, Head, Note, NoteId, NoteRevision, ResolvedNote, RevisionGraphError,
    RevisionId, resolve_heads, validate_revision_graph,
};

const NOTE_PREFIX: &str = "note/";

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
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkspaceSnapshot {
    pub notes: Vec<Note>,
    pub resolved: Vec<ResolvedNote>,
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

    use crate::domain::{Frontmatter, FrontmatterValue};
    use crate::iroh_node::IrohNode;
    use crate::{Hlc, SchemaVersion};

    use super::*;

    #[tokio::test]
    async fn committed_records_resolve_and_rebuild_the_index() -> anyhow::Result<()> {
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
        assert_eq!(index.all()?.first().expect("indexed note").title, "Stored");
        node.shutdown().await?;
        Ok(())
    }
}
