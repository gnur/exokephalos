//! Automerge-backed workspace compatibility facade over authenticated Iroh protocols.

use std::path::{Path, PathBuf};
use std::pin::Pin;

use anyhow::{Context as _, Result, bail};
use iroh::EndpointId;
#[cfg(any(test, feature = "test-utils"))]
use iroh::RelayMap;

use crate::automerge_node::{AutomergeNode, AutomergeWorkspace, AutomergeWorkspaceEvent};
use crate::membership::{Member, PeerId};
use crate::peer_protocol::{JoinRequest, JoinResponse, WorkspaceInvitation};
use crate::{ActorId, ConfigRevision, DeviceRecord, Head, NoteRevision, Tombstone, WorkspaceId};

#[cfg(test)]
pub(crate) static IROH_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn validate_writable_ticket(ticket: &str) -> Result<()> {
    WorkspaceInvitation::decode(ticket)?;
    Ok(())
}

pub fn writable_ticket_workspace_id(ticket: &str) -> Result<String> {
    Ok(WorkspaceInvitation::decode(ticket)?.workspace_id)
}

#[derive(Debug)]
pub struct IrohNode {
    inner: AutomergeNode,
    state_dir: PathBuf,
}

impl IrohNode {
    pub async fn persistent(state_dir: impl AsRef<Path>) -> Result<Self> {
        let state_dir = state_dir.as_ref();
        let saved_peer_id = state_dir.join("identity/peer-id");
        let peer_id = match std::fs::read_to_string(&saved_peer_id) {
            Ok(value) => PeerId::parse(value.trim())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let hostname = hostname::get()
                    .context("read system hostname")?
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("system hostname is not valid UTF-8"))?;
                PeerId::parse(hostname)?
            }
            Err(error) => {
                return Err(error).with_context(|| format!("read {}", saved_peer_id.display()));
            }
        };
        Self::persistent_with_peer(state_dir, peer_id).await
    }

    pub async fn persistent_with_peer(
        state_dir: impl AsRef<Path>,
        peer_id: PeerId,
    ) -> Result<Self> {
        let state_dir = state_dir.as_ref().to_path_buf();
        let inner = AutomergeNode::persistent(&state_dir, peer_id).await?;
        Ok(Self { inner, state_dir })
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn persistent_with_relay_map(
        state_dir: impl AsRef<Path>,
        relay_map: RelayMap,
    ) -> Result<Self> {
        let state_dir = state_dir.as_ref().to_path_buf();
        let peer_id = PeerId::parse(format!(
            "peer-{}",
            &blake3::hash(state_dir.as_os_str().as_encoded_bytes()).to_hex()[..12]
        ))?;
        Self::persistent_with_peer_and_relay_map(state_dir, peer_id, relay_map).await
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn persistent_with_peer_and_relay_map(
        state_dir: impl AsRef<Path>,
        peer_id: PeerId,
        relay_map: RelayMap,
    ) -> Result<Self> {
        let state_dir = state_dir.as_ref().to_path_buf();
        let inner =
            AutomergeNode::persistent_with_relay_map(&state_dir, peer_id, relay_map).await?;
        Ok(Self { inner, state_dir })
    }

    #[must_use]
    pub fn endpoint_id(&self) -> EndpointId {
        self.inner.endpoint_id()
    }

    #[must_use]
    pub fn author_id(&self) -> ActorId {
        ActorId::new(self.inner.membership_fingerprint())
    }

    #[must_use]
    pub fn peer_id(&self) -> &PeerId {
        self.inner.peer_id()
    }

    #[must_use]
    pub fn membership_fingerprint(&self) -> String {
        self.inner.membership_fingerprint()
    }

    #[must_use]
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub async fn workspace_ids(&self) -> Result<Vec<String>> {
        Ok(self.inner.workspace_ids().await)
    }

    pub async fn create_workspace(&self) -> Result<IrohWorkspace> {
        Ok(self.workspace(self.inner.create_workspace().await?))
    }

    pub async fn open_workspace(&self, id: &WorkspaceId) -> Result<Option<IrohWorkspace>> {
        self.open_workspace_str(id.as_str()).await
    }

    pub async fn open_workspace_str(&self, id: &str) -> Result<Option<IrohWorkspace>> {
        Ok(self
            .inner
            .open_workspace(id)
            .await
            .map(|workspace| self.workspace(workspace)))
    }

    pub async fn import_workspace(&self, ticket: &str) -> Result<IrohWorkspace> {
        self.import_writable_workspace(ticket).await
    }

    pub async fn import_writable_workspace(&self, ticket: &str) -> Result<IrohWorkspace> {
        validate_writable_ticket(ticket)?;
        match self.inner.request_join(ticket).await? {
            JoinResponse::Approved { .. } => {
                let workspace = self.inner.import_approved_workspace(ticket).await?;
                workspace
                    .add_invitation_peers(&WorkspaceInvitation::decode(ticket)?)
                    .await?;
                Ok(self.workspace(workspace))
            }
            JoinResponse::Pending => bail!("workspace membership request is pending approval"),
            JoinResponse::Rejected => bail!("workspace membership request was rejected"),
        }
    }

    pub async fn import_writable_workspace_synced(&self, ticket: &str) -> Result<IrohWorkspace> {
        let workspace = self.import_writable_workspace(ticket).await?;
        workspace.sync_and_wait(ticket).await?;
        Ok(workspace)
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.inner.shutdown().await
    }

    fn workspace(&self, inner: AutomergeWorkspace) -> IrohWorkspace {
        IrohWorkspace {
            inner,
            author: self.author_id(),
            endpoint_id: self.endpoint_id(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct IrohWorkspace {
    inner: AutomergeWorkspace,
    author: ActorId,
    endpoint_id: EndpointId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceEvent {
    ContentChanged,
    StatusChanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignedWorkspaceValue {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub author: String,
}

impl IrohWorkspace {
    #[must_use]
    pub fn id(&self) -> WorkspaceId {
        WorkspaceId::new(self.inner.id())
    }

    #[must_use]
    pub fn author_id(&self) -> ActorId {
        self.author.clone()
    }

    #[must_use]
    pub fn peer_id(&self) -> &PeerId {
        self.inner.identity_peer_id()
    }

    #[must_use]
    pub fn membership_fingerprint(&self) -> String {
        self.inner.author_fingerprint()
    }

    #[allow(clippy::unused_async)]
    pub async fn share(&self, _writable: bool) -> Result<String> {
        self.inner.invitation()
    }

    pub async fn put(&self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Result<String> {
        let key = String::from_utf8(key.into()).context("workspace record key is not UTF-8")?;
        let value = value.into();
        let hash = blake3::hash(&value).to_hex().to_string();
        self.inner.put(&key, value).await?;
        Ok(hash)
    }

    pub async fn put_blob(
        &self,
        key: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
    ) -> Result<String> {
        self.put(key, value).await
    }

    pub async fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<Vec<u8>>> {
        let key = std::str::from_utf8(key.as_ref()).context("workspace record key is not UTF-8")?;
        self.inner.get(key).await
    }

    pub async fn get_signed(&self, key: impl AsRef<[u8]>) -> Result<Option<SignedWorkspaceValue>> {
        let key = key.as_ref();
        let Some(value) = self.get(key).await? else {
            return Ok(None);
        };
        Ok(Some(SignedWorkspaceValue {
            key: key.to_vec(),
            author: record_author(key, &value).unwrap_or_else(|| self.author.to_string()),
            value,
        }))
    }

    pub async fn list(&self, prefix: impl AsRef<[u8]>) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        Ok(self
            .list_signed(prefix)
            .await?
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect())
    }

    pub async fn list_signed(&self, prefix: impl AsRef<[u8]>) -> Result<Vec<SignedWorkspaceValue>> {
        let prefix =
            std::str::from_utf8(prefix.as_ref()).context("workspace record prefix is not UTF-8")?;
        Ok(self
            .inner
            .scan(prefix)
            .await?
            .into_iter()
            .map(|(key, value)| SignedWorkspaceValue {
                author: record_author(key.as_bytes(), &value)
                    .unwrap_or_else(|| self.author.to_string()),
                key: key.into_bytes(),
                value,
            })
            .collect())
    }

    #[allow(clippy::unused_async)]
    pub async fn subscribe(
        &self,
    ) -> Result<Pin<Box<dyn futures_lite::Stream<Item = Result<WorkspaceEvent>> + Send + 'static>>>
    {
        let receiver = self.inner.subscribe();
        Ok(Box::pin(futures_lite::stream::unfold(
            receiver,
            |mut receiver| async move {
                match receiver.recv().await {
                    Ok(
                        AutomergeWorkspaceEvent::ContentChanged
                        | AutomergeWorkspaceEvent::MembershipChanged,
                    ) => Some((Ok(WorkspaceEvent::ContentChanged), receiver)),
                    Ok(AutomergeWorkspaceEvent::StatusChanged)
                    | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        Some((Ok(WorkspaceEvent::StatusChanged), receiver))
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
                }
            },
        )))
    }

    pub async fn start_sync(&self, ticket: &str) -> Result<()> {
        let invitation = WorkspaceInvitation::decode(ticket)?;
        self.inner.add_invitation_peers(&invitation).await
    }

    pub async fn sync_and_wait(&self, ticket: &str) -> Result<()> {
        self.start_sync(ticket).await
    }

    pub async fn resume_sync(&self) -> Result<()> {
        self.inner.sync().await
    }

    pub async fn pending_requests(&self) -> Vec<JoinRequest> {
        self.inner.pending_requests().await
    }

    pub async fn approve_peer(&self, public_key: &[u8; 32]) -> Result<()> {
        self.inner.approve(public_key).await?;
        Ok(())
    }

    pub async fn reject_peer(&self, public_key: &[u8; 32]) -> Result<()> {
        self.inner.reject(public_key).await?;
        Ok(())
    }

    pub async fn remove_peer(&self, public_key: &[u8; 32], reason: Option<String>) -> Result<()> {
        self.inner.remove(public_key, reason).await?;
        Ok(())
    }

    pub async fn members(&self) -> Vec<Member> {
        self.inner.members().await
    }

    #[must_use]
    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint_id
    }
}

fn record_author(key: &[u8], value: &[u8]) -> Option<String> {
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
    if key.starts_with("device/") {
        return ciborium::from_reader::<DeviceRecord, _>(value)
            .ok()
            .map(|record| {
                record
                    .retired_at
                    .as_ref()
                    .map_or(record.author_id.clone(), |cutoff| cutoff.actor_id.clone())
                    .to_string()
            });
    }
    None
}
