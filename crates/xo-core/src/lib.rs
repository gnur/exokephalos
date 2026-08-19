//! Shared domain, storage, and synchronization contracts for exokephalos.

pub mod authenticated_change;
#[cfg(feature = "iroh-sync")]
pub mod automerge_node;
pub mod automerge_store;
#[cfg(feature = "native")]
pub mod backup;
pub mod behavior;
#[cfg(feature = "native")]
pub mod central_replica;
pub mod central_sync;
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
pub mod membership;
#[cfg(feature = "peer-protocol")]
pub mod peer_protocol;
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
#[cfg(feature = "iroh-sync")]
pub mod workspace_projection;

pub use domain::{
    ActorId, AssetId, AssetRecord, ConfigRevision, Conflict, DeviceRecord, DomainError, Head, Note,
    NoteId, NoteRevision, RevisionId, SchemaVersion, Tombstone, WorkspaceDescriptor, WorkspaceId,
};
pub use hlc::{Hlc, HlcClock};
pub use membership::{MembershipIdentity, PeerId};
pub use resolution::{ResolvedNote, RevisionGraphError, resolve_heads, validate_revision_graph};

/// Version of the replicated record schema written by this build.
pub const CURRENT_SCHEMA: SchemaVersion = SchemaVersion(1);
