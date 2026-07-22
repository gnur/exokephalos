//! Shared domain, storage, and synchronization contracts for exokephalos.

pub mod domain;
pub mod encryption;
pub mod hlc;
pub mod id;
#[cfg(feature = "iroh-sync")]
pub mod iroh_node;
pub mod local_index;
pub mod markdown;
pub mod projection;
pub mod resolution;
pub mod wikilink;

pub use domain::{
    ActorId, AssetId, AssetRecord, Conflict, DeviceRecord, DomainError, Head, Note, NoteId,
    NoteRevision, RevisionId, SchemaVersion, Tombstone, WorkspaceDescriptor, WorkspaceId,
};
pub use hlc::{Hlc, HlcClock};
pub use resolution::{ResolvedNote, RevisionGraphError, resolve_heads, validate_revision_graph};

/// Version of the replicated record schema written by this build.
pub const CURRENT_SCHEMA: SchemaVersion = SchemaVersion(1);
