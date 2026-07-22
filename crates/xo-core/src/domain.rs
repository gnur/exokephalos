use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::hlc::Hlc;

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
    /// Serialize the revision in the stable byte representation used for hashing and storage.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>> {
        let mut bytes = Vec::new();
        ciborium::into_writer(self, &mut bytes)?;
        Ok(bytes)
    }

    /// Calculate the immutable content-addressed identity of this revision.
    pub fn id(&self) -> Result<RevisionId, ciborium::ser::Error<std::io::Error>> {
        Ok(RevisionId(
            blake3::hash(&self.canonical_bytes()?).to_hex().to_string(),
        ))
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
pub struct Tombstone {
    pub schema: SchemaVersion,
    pub target_id: String,
    pub author_id: ActorId,
    pub revision_id: RevisionId,
    pub hlc: Hlc,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub schema: SchemaVersion,
    pub endpoint_id: String,
    pub author_id: ActorId,
    pub label: String,
    pub capabilities: BTreeSet<String>,
    pub last_seen_ms: Option<u64>,
    pub retired_at: Option<Hlc>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceDescriptor {
    pub schema: SchemaVersion,
    pub workspace_id: WorkspaceId,
    pub docs_ticket: String,
    pub bootstrap_peers: Vec<String>,
    pub relay_mode: String,
    pub encrypted_workspace_key: Option<String>,
    pub read_only: bool,
}
