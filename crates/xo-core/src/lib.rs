//! Shared domain, storage, and synchronization contracts for exokephalos.

pub mod automerge_store;
pub mod behavior;
#[cfg(feature = "native")]
pub mod central_replica;
pub mod central_sync;
pub mod domain;
#[cfg(feature = "native")]
pub mod encryption;
pub mod hlc;
pub mod id;
#[cfg(feature = "native")]
pub mod local_index;
pub mod markdown;
#[cfg(feature = "native")]
pub mod projection;
pub mod record_workspace;
#[cfg(feature = "native")]
pub mod records;
pub mod resolution;
#[cfg(feature = "steel")]
pub mod steel_runtime;
#[cfg(feature = "native")]
pub mod sync_state;
pub mod timestamp;
#[cfg(feature = "native")]
pub mod url_capture;
pub mod version;
#[cfg(feature = "native")]
pub mod watcher;
#[cfg(feature = "native")]
pub mod wikilink;

pub use central_sync::ClientId;
pub use domain::{
    ActorId, AssetId, AssetRecord, ConfigRevision, Conflict, DomainError, Head, Note, NoteId,
    NoteRevision, RevisionId, SchemaVersion, Tombstone, WorkspaceId,
};
pub use hlc::{Hlc, HlcClock};
pub use resolution::{ResolvedNote, RevisionGraphError, resolve_heads, validate_revision_graph};

/// Version of the replicated record schema written by this build.
pub const CURRENT_SCHEMA: SchemaVersion = SchemaVersion(1);
