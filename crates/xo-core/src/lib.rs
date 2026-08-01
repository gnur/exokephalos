//! Shared domain, storage, and synchronization contracts for exokephalos.

#[cfg(feature = "native")]
pub mod backup;
pub mod behavior;
pub mod domain;
#[cfg(feature = "native")]
pub mod encryption;
pub mod hlc;
pub mod id;
#[cfg(feature = "iroh-sync")]
pub mod iroh_node;
#[cfg(feature = "native")]
pub mod local_index;
pub mod markdown;
#[cfg(feature = "native")]
pub mod projection;
#[cfg(feature = "iroh-sync")]
pub mod records;
pub mod resolution;
#[cfg(feature = "iroh-sync")]
pub mod rotation;
#[cfg(feature = "steel")]
pub mod steel_runtime;
#[cfg(feature = "native")]
pub mod sync_state;
pub mod version;
#[cfg(feature = "native")]
pub mod watcher;
#[cfg(feature = "native")]
pub mod wikilink;
#[cfg(feature = "iroh-sync")]
pub mod workspace_projection;

pub use domain::{
    ActorId, AssetId, AssetRecord, ConfigRevision, Conflict, DeviceRecord, DomainError, Head, Note,
    NoteId, NoteRevision, RevisionId, SchemaVersion, Tombstone, WorkspaceDescriptor, WorkspaceId,
};
pub use hlc::{Hlc, HlcClock};
pub use resolution::{ResolvedNote, RevisionGraphError, resolve_heads, validate_revision_graph};

/// Version of the replicated record schema written by this build.
pub const CURRENT_SCHEMA: SchemaVersion = SchemaVersion(1);
