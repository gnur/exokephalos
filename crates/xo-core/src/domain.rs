use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::hlc::Hlc;
use crate::{CURRENT_SCHEMA, id};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(
            Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

string_id!(WorkspaceId);
string_id!(ActorId);
string_id!(NoteId);
string_id!(RevisionId);
string_id!(AssetId);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(pub u16);

/// YAML-compatible values with deterministic map ordering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FrontmatterValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Sequence(Vec<Self>),
    Mapping(BTreeMap<String, Self>),
}

pub type Frontmatter = BTreeMap<String, FrontmatterValue>;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DomainError {
    #[error("unsupported schema version {0}")]
    UnsupportedSchema(u16),
    #[error("{field} must not be empty")]
    EmptyField { field: &'static str },
    #[error("invalid note ID: {0}")]
    InvalidNoteId(String),
    #[error("invalid materialized path: {0}")]
    InvalidPath(String),
    #[error("asset path must be below assets/: {0}")]
    InvalidAssetPath(String),
    #[error("frontmatter contains a non-finite number")]
    NonFiniteNumber,
    #[error("revision author does not match HLC actor")]
    AuthorMismatch,
    #[error("revision serialization failed: {0}")]
    Serialization(String),
}

impl FrontmatterValue {
    pub fn validate(&self) -> Result<(), DomainError> {
        match self {
            Self::Float(value) if !value.is_finite() => Err(DomainError::NonFiniteNumber),
            Self::Sequence(values) => values.iter().try_for_each(Self::validate),
            Self::Mapping(values) => values.values().try_for_each(Self::validate),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: NoteId,
    pub frontmatter: Frontmatter,
    pub body: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteRevision {
    pub schema: SchemaVersion,
    pub note_id: NoteId,
    pub frontmatter: Frontmatter,
    pub body: String,
    pub materialized_path: String,
    pub hlc: Hlc,
    pub author_id: ActorId,
    pub predecessors: BTreeSet<RevisionId>,
    pub deleted: bool,
}

impl NoteRevision {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_schema(self.schema)?;
        if !id::is_valid(self.note_id.as_str()) {
            return Err(DomainError::InvalidNoteId(self.note_id.to_string()));
        }
        if self.author_id.as_str().is_empty() {
            return Err(DomainError::EmptyField { field: "author_id" });
        }
        if self.author_id != self.hlc.actor_id {
            return Err(DomainError::AuthorMismatch);
        }
        validate_relative_path(&self.materialized_path)?;
        if Path::new(&self.materialized_path)
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == "assets")
        {
            return Err(DomainError::InvalidPath(self.materialized_path.clone()));
        }
        self.frontmatter
            .values()
            .try_for_each(FrontmatterValue::validate)
    }

    /// Serialize the revision in the stable byte representation used for hashing and storage.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DomainError> {
        self.validate()?;
        let mut bytes = Vec::new();
        ciborium::into_writer(self, &mut bytes)
            .map_err(|error| DomainError::Serialization(error.to_string()))?;
        Ok(bytes)
    }

    /// Calculate the immutable content-addressed identity of this revision.
    pub fn id(&self) -> Result<RevisionId, DomainError> {
        Ok(RevisionId(
            blake3::hash(&self.canonical_bytes()?).to_hex().to_string(),
        ))
    }

    pub fn revise(
        &self,
        frontmatter: Frontmatter,
        body: impl Into<String>,
        materialized_path: impl Into<String>,
        hlc: Hlc,
        author_id: ActorId,
        deleted: bool,
    ) -> Result<Self, DomainError> {
        let predecessor = self.id()?;
        let revision = Self {
            schema: CURRENT_SCHEMA,
            note_id: self.note_id.clone(),
            frontmatter,
            body: body.into(),
            materialized_path: materialized_path.into(),
            hlc,
            author_id,
            predecessors: BTreeSet::from([predecessor]),
            deleted,
        };
        revision.validate()?;
        Ok(revision)
    }

    pub fn delete(&self, hlc: Hlc, author_id: ActorId) -> Result<Self, DomainError> {
        self.revise(
            self.frontmatter.clone(),
            self.body.clone(),
            self.materialized_path.clone(),
            hlc,
            author_id,
            true,
        )
    }

    pub fn restore(&self, hlc: Hlc, author_id: ActorId) -> Result<Self, DomainError> {
        self.revise(
            self.frontmatter.clone(),
            self.body.clone(),
            self.materialized_path.clone(),
            hlc,
            author_id,
            false,
        )
    }

    pub fn rename(
        &self,
        path: impl Into<String>,
        hlc: Hlc,
        author_id: ActorId,
    ) -> Result<Self, DomainError> {
        self.revise(
            self.frontmatter.clone(),
            self.body.clone(),
            path,
            hlc,
            author_id,
            self.deleted,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Head {
    pub note_id: NoteId,
    pub author_id: ActorId,
    pub revision_id: RevisionId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    pub note_id: NoteId,
    pub winning_revision: RevisionId,
    pub concurrent_revisions: BTreeSet<RevisionId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AssetRecord {
    pub schema: SchemaVersion,
    pub id: AssetId,
    pub blob_hash: String,
    pub mime: String,
    pub size: u64,
    pub materialized_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConfigRevision {
    pub schema: SchemaVersion,
    pub path: String,
    pub blob_hash: String,
    pub size: u64,
    pub hlc: Hlc,
    pub author_id: ActorId,
    pub predecessors: BTreeSet<RevisionId>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Tombstone {
    pub schema: SchemaVersion,
    pub target_id: String,
    pub author_id: ActorId,
    pub revision_id: RevisionId,
    pub hlc: Hlc,
}

impl AssetRecord {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_schema(self.schema)?;
        if self.id.as_str().is_empty() {
            return Err(DomainError::EmptyField { field: "asset_id" });
        }
        if self.blob_hash.is_empty() {
            return Err(DomainError::EmptyField { field: "blob_hash" });
        }
        if self.mime.is_empty() {
            return Err(DomainError::EmptyField { field: "mime" });
        }
        validate_relative_path(&self.materialized_path)?;
        if Path::new(&self.materialized_path)
            .components()
            .next()
            .is_none_or(|component| component.as_os_str() != "assets")
        {
            return Err(DomainError::InvalidAssetPath(
                self.materialized_path.clone(),
            ));
        }
        Ok(())
    }
}

impl ConfigRevision {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_schema(self.schema)?;
        validate_relative_path(&self.path)?;
        if self.blob_hash.is_empty() {
            return Err(DomainError::EmptyField { field: "blob_hash" });
        }
        if self.author_id != self.hlc.actor_id {
            return Err(DomainError::AuthorMismatch);
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DomainError> {
        self.validate()?;
        let mut bytes = Vec::new();
        ciborium::into_writer(self, &mut bytes)
            .map_err(|error| DomainError::Serialization(error.to_string()))?;
        Ok(bytes)
    }

    pub fn id(&self) -> Result<RevisionId, DomainError> {
        Ok(RevisionId::new(
            blake3::hash(&self.canonical_bytes()?).to_hex().to_string(),
        ))
    }
}

impl Head {
    pub fn validate(&self) -> Result<(), DomainError> {
        if !id::is_valid(self.note_id.as_str()) {
            return Err(DomainError::InvalidNoteId(self.note_id.to_string()));
        }
        if self.author_id.as_str().is_empty() {
            return Err(DomainError::EmptyField { field: "author_id" });
        }
        if self.revision_id.as_str().is_empty() {
            return Err(DomainError::EmptyField {
                field: "revision_id",
            });
        }
        Ok(())
    }
}

impl Tombstone {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_schema(self.schema)?;
        if self.target_id.is_empty() {
            return Err(DomainError::EmptyField { field: "target_id" });
        }
        if self.author_id != self.hlc.actor_id {
            return Err(DomainError::AuthorMismatch);
        }
        Ok(())
    }
}

fn validate_schema(schema: SchemaVersion) -> Result<(), DomainError> {
    if schema == CURRENT_SCHEMA {
        Ok(())
    } else {
        Err(DomainError::UnsupportedSchema(schema.0))
    }
}

fn validate_relative_path(path: &str) -> Result<(), DomainError> {
    let parsed = Path::new(path);
    let invalid = path.is_empty()
        || parsed.is_absolute()
        || parsed.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        || parsed
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == ".xo");
    if invalid {
        Err(DomainError::InvalidPath(path.to_owned()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp(actor: &str, logical: u32) -> Hlc {
        Hlc {
            physical_ms: 100,
            logical,
            actor_id: ActorId::new(actor),
        }
    }

    fn revision() -> NoteRevision {
        NoteRevision {
            schema: CURRENT_SCHEMA,
            note_id: NoteId::new("note002"),
            frontmatter: Frontmatter::from([(
                "title".to_owned(),
                FrontmatterValue::String("A note".to_owned()),
            )]),
            body: "body".to_owned(),
            materialized_path: "notes/a-note.md".to_owned(),
            hlc: timestamp("device-a", 0),
            author_id: ActorId::new("device-a"),
            predecessors: BTreeSet::new(),
            deleted: false,
        }
    }

    #[test]
    fn rejects_non_finite_frontmatter_before_hashing() {
        let mut revision = revision();
        revision
            .frontmatter
            .insert("broken".to_owned(), FrontmatterValue::Float(f64::NAN));
        assert_eq!(revision.id(), Err(DomainError::NonFiniteNumber));
    }

    #[test]
    fn delete_restore_and_rename_form_a_predecessor_chain() {
        let base = revision();
        let base_id = base.id().unwrap();
        let deleted = base
            .delete(timestamp("device-b", 1), ActorId::new("device-b"))
            .unwrap();
        assert!(deleted.deleted);
        assert_eq!(deleted.predecessors, BTreeSet::from([base_id]));

        let deleted_id = deleted.id().unwrap();
        let restored = deleted
            .restore(timestamp("device-b", 2), ActorId::new("device-b"))
            .unwrap();
        assert!(!restored.deleted);
        assert_eq!(restored.predecessors, BTreeSet::from([deleted_id]));

        let restored_id = restored.id().unwrap();
        let renamed = restored
            .rename(
                "archive/a-note.md",
                timestamp("device-c", 3),
                ActorId::new("device-c"),
            )
            .unwrap();
        assert_eq!(renamed.materialized_path, "archive/a-note.md");
        assert_eq!(renamed.predecessors, BTreeSet::from([restored_id]));
    }

    #[test]
    fn rejects_paths_outside_the_projection() {
        let revision = revision();
        assert!(matches!(
            revision.rename(
                "../escape.md",
                timestamp("device-a", 1),
                ActorId::new("device-a")
            ),
            Err(DomainError::InvalidPath(_))
        ));
    }
}
