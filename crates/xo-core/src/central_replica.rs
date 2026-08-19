//! Durable local Automerge replica used by centralized clients.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use automerge::sync::State as SyncState;
use tokio::sync::{Mutex, RwLock, broadcast};

use crate::ActorId;
use crate::automerge_store::PersistentAutomergeStore;
use crate::record_workspace::{AuthoredWorkspaceValue, RecordWorkspace, record_author};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicaEvent {
    ContentChanged,
    StatusChanged,
}

#[derive(Debug)]
pub struct CentralReplica {
    workspace_id: String,
    actor: ActorId,
    store: Mutex<PersistentAutomergeStore>,
    events: broadcast::Sender<ReplicaEvent>,
    connected_clients: RwLock<Vec<String>>,
}

impl CentralReplica {
    pub fn open(
        state_dir: &Path,
        workspace_id: &str,
        actor: ActorId,
        automerge_actor: &[u8],
    ) -> Result<Arc<Self>> {
        let store = PersistentAutomergeStore::open_or_create(
            &state_dir.join("replica"),
            workspace_id,
            automerge_actor,
        )?;
        let (events, _) = broadcast::channel(256);
        Ok(Arc::new(Self {
            workspace_id: workspace_id.to_owned(),
            actor,
            store: Mutex::new(store),
            events,
            connected_clients: RwLock::new(Vec::new()),
        }))
    }

    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ReplicaEvent> {
        self.events.subscribe()
    }

    pub async fn generate_sync_message(&self, state: &mut SyncState) -> Option<Vec<u8>> {
        self.store.lock().await.generate_sync_message(state)
    }

    pub async fn receive_sync_message(
        &self,
        state: &mut SyncState,
        message: &[u8],
    ) -> Result<bool> {
        let changed = self
            .store
            .lock()
            .await
            .receive_sync_message(state, message)?;
        if changed {
            let _ = self.events.send(ReplicaEvent::ContentChanged);
        }
        Ok(changed)
    }

    pub async fn set_connected_clients(&self, mut clients: Vec<String>) {
        clients.sort();
        clients.dedup();
        let mut current = self.connected_clients.write().await;
        if *current != clients {
            *current = clients;
            let _ = self.events.send(ReplicaEvent::StatusChanged);
        }
    }

    pub async fn connected_clients(&self) -> Vec<String> {
        self.connected_clients.read().await.clone()
    }

    fn changed(&self) {
        let _ = self.events.send(ReplicaEvent::ContentChanged);
    }
}

impl RecordWorkspace for CentralReplica {
    fn record_actor_id(&self) -> ActorId {
        self.actor.clone()
    }

    async fn put_record(
        &self,
        key: impl Into<Vec<u8>> + Send,
        value: impl Into<Vec<u8>> + Send,
    ) -> Result<String> {
        let key = String::from_utf8(key.into()).context("workspace record key is not UTF-8")?;
        let value = value.into();
        let hash = blake3::hash(&value).to_hex().to_string();
        self.store.lock().await.put(&key, value)?;
        self.changed();
        Ok(hash)
    }

    async fn put_blob_record(
        &self,
        key: impl Into<Vec<u8>> + Send,
        value: impl Into<Vec<u8>> + Send,
    ) -> Result<String> {
        self.put_record(key, value).await
    }

    async fn get_record(&self, key: impl AsRef<[u8]> + Send) -> Result<Option<Vec<u8>>> {
        let key = std::str::from_utf8(key.as_ref()).context("workspace record key is not UTF-8")?;
        Ok(self.store.lock().await.store().get(key)?)
    }

    async fn get_authored_record(
        &self,
        key: impl AsRef<[u8]> + Send,
    ) -> Result<Option<AuthoredWorkspaceValue>> {
        let key = key.as_ref();
        let Some(value) = self.get_record(key).await? else {
            return Ok(None);
        };
        Ok(Some(AuthoredWorkspaceValue {
            key: key.to_vec(),
            author: record_author(key, &value).unwrap_or_else(|| self.actor.to_string()),
            value,
        }))
    }

    async fn list_records(
        &self,
        prefix: impl AsRef<[u8]> + Send,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        Ok(self
            .list_authored_records(prefix)
            .await?
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect())
    }

    async fn list_authored_records(
        &self,
        prefix: impl AsRef<[u8]> + Send,
    ) -> Result<Vec<AuthoredWorkspaceValue>> {
        let prefix =
            std::str::from_utf8(prefix.as_ref()).context("workspace record prefix is not UTF-8")?;
        Ok(self
            .store
            .lock()
            .await
            .store()
            .scan(prefix)?
            .into_iter()
            .map(|(key, value)| AuthoredWorkspaceValue {
                author: record_author(key.as_bytes(), &value)
                    .unwrap_or_else(|| self.actor.to_string()),
                key: key.into_bytes(),
                value,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_are_durable_and_emit_changes() {
        let directory = tempfile::tempdir().unwrap();
        let replica = CentralReplica::open(
            directory.path(),
            "workspace-a",
            ActorId::new("client-a"),
            b"automerge-client-a",
        )
        .unwrap();
        let mut events = replica.subscribe();
        replica.put_record("note/a", vec![1, 2, 3]).await.unwrap();
        assert_eq!(events.recv().await.unwrap(), ReplicaEvent::ContentChanged);
        drop(replica);
        let reopened = CentralReplica::open(
            directory.path(),
            "workspace-a",
            ActorId::new("client-a"),
            b"automerge-client-a",
        )
        .unwrap();
        assert_eq!(
            reopened.get_record("note/a").await.unwrap(),
            Some(vec![1, 2, 3])
        );
    }
}
