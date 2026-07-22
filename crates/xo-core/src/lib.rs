//! Shared domain, storage, and synchronization contracts for exokephalos.

pub mod domain;
pub mod encryption;
pub mod hlc;
pub mod id;
#[cfg(feature = "iroh-sync")]
pub mod iroh_node;
pub mod markdown;
pub mod resolution;
pub mod wikilink;

pub use domain::{
    ActorId, AssetId, AssetRecord, Conflict, DeviceRecord, Head, Note, NoteId, NoteRevision,
    RevisionId, SchemaVersion, Tombstone, WorkspaceDescriptor, WorkspaceId,
};
pub use hlc::{Hlc, HlcClock};
pub use resolution::{ResolvedNote, resolve_heads};

/// Version of the replicated record schema written by this build.
pub const CURRENT_SCHEMA: SchemaVersion = SchemaVersion(1);
