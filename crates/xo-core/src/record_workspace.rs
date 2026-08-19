//! Storage interface used by xo's typed immutable record repository.

use anyhow::Result;

use crate::{ActorId, ConfigRevision, Head, NoteRevision, Tombstone};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoredWorkspaceValue {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub author: String,
}

#[allow(async_fn_in_trait)]
pub trait RecordWorkspace: Send + Sync {
    fn record_actor_id(&self) -> ActorId;

    async fn put_record(
        &self,
        key: impl Into<Vec<u8>> + Send,
        value: impl Into<Vec<u8>> + Send,
    ) -> Result<String>;

    async fn put_blob_record(
        &self,
        key: impl Into<Vec<u8>> + Send,
        value: impl Into<Vec<u8>> + Send,
    ) -> Result<String>;

    async fn get_record(&self, key: impl AsRef<[u8]> + Send) -> Result<Option<Vec<u8>>>;

    async fn get_authored_record(
        &self,
        key: impl AsRef<[u8]> + Send,
    ) -> Result<Option<AuthoredWorkspaceValue>>;

    async fn list_records(
        &self,
        prefix: impl AsRef<[u8]> + Send,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>>;

    async fn list_authored_records(
        &self,
        prefix: impl AsRef<[u8]> + Send,
    ) -> Result<Vec<AuthoredWorkspaceValue>>;
}

#[must_use]
pub fn record_author(key: &[u8], value: &[u8]) -> Option<String> {
    let key = std::str::from_utf8(key).ok()?;
    if key.contains("/revision/") {
        return ciborium::from_reader::<NoteRevision, _>(value)
            .ok()
            .map(|record| record.author_id.to_string());
    }
    if key.contains("/head/") {
        return ciborium::from_reader::<Head, _>(value)
            .ok()
            .map(|record| record.author_id.to_string());
    }
    if key.starts_with("config/") {
        return ciborium::from_reader::<ConfigRevision, _>(value)
            .ok()
            .map(|record| record.author_id.to_string());
    }
    if key.starts_with("tombstone/") {
        return ciborium::from_reader::<Tombstone, _>(value)
            .ok()
            .map(|record| record.author_id.to_string());
    }
    None
}
