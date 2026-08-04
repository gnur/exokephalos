//! Persistent native Iroh protocol composition.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_lite::StreamExt;
use iroh::endpoint::presets;
use iroh::protocol::Router;
#[cfg(any(test, feature = "test-utils"))]
use iroh::tls::CaRootsConfig;
use iroh::{Endpoint, EndpointId, RelayMap, SecretKey};
use iroh_blobs::api::Store as BlobStore;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::{ALPN as BLOBS_ALPN, BlobsProtocol};
use iroh_docs::api::protocol::{AddrInfoOptions, ShareMode};
use iroh_docs::api::{Doc, DocsApi};
use iroh_docs::engine::LiveEvent;
use iroh_docs::protocol::Docs;
use iroh_docs::store::Query;
use iroh_docs::{ALPN as DOCS_ALPN, AuthorId, DocTicket, NamespaceId};
use iroh_gossip::ALPN as GOSSIP_ALPN;
use iroh_gossip::net::Gossip;

const ENDPOINT_KEY_FILE: &str = "endpoint.key";

#[cfg(test)]
pub(crate) static IROH_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn validate_writable_ticket(ticket: &str) -> Result<()> {
    let ticket = DocTicket::from_str(ticket).context("parse workspace ticket")?;
    if ticket.capability.secret_key().is_err() {
        bail!("workspace ticket is read-only; a writable ticket is required");
    }
    Ok(())
}

pub fn writable_ticket_workspace_id(ticket: &str) -> Result<String> {
    let ticket = DocTicket::from_str(ticket).context("parse workspace ticket")?;
    if ticket.capability.secret_key().is_err() {
        bail!("workspace ticket is read-only; a writable ticket is required");
    }
    Ok(ticket.capability.id().to_string())
}

/// A persistent endpoint hosting Docs, Blobs, and Gossip on one router.
#[derive(Debug)]
pub struct IrohNode {
    router: Router,
    docs: Docs,
    blobs: BlobStore,
    author: AuthorId,
    state_dir: PathBuf,
}

impl IrohNode {
    pub async fn persistent(state_dir: impl AsRef<Path>) -> Result<Self> {
        Self::persistent_inner(state_dir, None).await
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn persistent_with_relay_map(
        state_dir: impl AsRef<Path>,
        relay_map: RelayMap,
    ) -> Result<Self> {
        Self::persistent_inner(state_dir, Some(relay_map)).await
    }

    async fn persistent_inner(
        state_dir: impl AsRef<Path>,
        #[allow(unused_variables)] relay_map: Option<RelayMap>,
    ) -> Result<Self> {
        let state_dir = state_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&state_dir)
            .with_context(|| format!("create state directory {}", state_dir.display()))?;
        let secret_key = load_or_create_secret_key(&state_dir.join(ENDPOINT_KEY_FILE))?;
        #[allow(unused_mut)]
        let mut endpoint_builder = Endpoint::builder(presets::N0).secret_key(secret_key);
        #[cfg(any(test, feature = "test-utils"))]
        if let Some(relay_map) = relay_map {
            endpoint_builder = endpoint_builder
                .relay_mode(iroh::RelayMode::Custom(relay_map))
                .ca_roots_config(CaRootsConfig::insecure_skip_verify());
        }
        let endpoint = endpoint_builder
            .bind()
            .await
            .context("bind Iroh endpoint")?;

        let fs_store = FsStore::load(state_dir.join("blobs"))
            .await
            .context("open persistent blob store")?;
        let blobs = (*fs_store).clone();
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let docs_dir = state_dir.join("docs");
        std::fs::create_dir_all(&docs_dir)
            .with_context(|| format!("create docs directory {}", docs_dir.display()))?;
        let docs = Docs::persistent(docs_dir)
            .spawn(endpoint.clone(), blobs.clone(), gossip.clone())
            .await
            .context("open persistent docs store")?;
        let author = docs
            .author_default()
            .await
            .context("load default docs author")?;

        let router = Router::builder(endpoint)
            .accept(BLOBS_ALPN, BlobsProtocol::new(&blobs, None))
            .accept(GOSSIP_ALPN, gossip)
            .accept(DOCS_ALPN, docs.clone())
            .spawn();

        Ok(Self {
            router,
            docs,
            blobs,
            author,
            state_dir,
        })
    }

    #[must_use]
    pub fn endpoint_id(&self) -> EndpointId {
        self.router.endpoint().id()
    }

    #[must_use]
    pub fn author_id(&self) -> AuthorId {
        self.author
    }

    #[must_use]
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub async fn workspace_ids(&self) -> Result<Vec<String>> {
        let workspaces = self.docs.list().await.context("list workspaces")?;
        futures_lite::pin!(workspaces);
        let mut ids = Vec::new();
        while let Some(workspace) = workspaces.next().await {
            let (id, _) = workspace.context("read workspace listing")?;
            ids.push(id.to_string());
        }
        ids.sort();
        Ok(ids)
    }

    #[must_use]
    pub fn docs(&self) -> &DocsApi {
        self.docs.api()
    }

    pub async fn create_workspace(&self) -> Result<IrohWorkspace> {
        let doc = self.docs.create().await.context("create Docs namespace")?;
        Ok(self.workspace(doc))
    }

    pub async fn open_workspace(&self, id: NamespaceId) -> Result<Option<IrohWorkspace>> {
        Ok(self.docs.open(id).await?.map(|doc| self.workspace(doc)))
    }

    pub async fn open_workspace_str(&self, id: &str) -> Result<Option<IrohWorkspace>> {
        let id = NamespaceId::from_str(id).context("parse workspace ID")?;
        self.open_workspace(id).await
    }

    pub async fn import_workspace(&self, ticket: &str) -> Result<IrohWorkspace> {
        let ticket = DocTicket::from_str(ticket).context("parse workspace ticket")?;
        let doc = self
            .docs
            .import(ticket)
            .await
            .context("import workspace ticket")?;
        Ok(self.workspace(doc))
    }

    pub async fn import_writable_workspace(&self, ticket: &str) -> Result<IrohWorkspace> {
        validate_writable_ticket(ticket)?;
        let ticket = DocTicket::from_str(ticket).context("parse workspace ticket")?;
        let doc = self
            .docs
            .import(ticket)
            .await
            .context("import writable workspace ticket")?;
        Ok(self.workspace(doc))
    }

    /// Import a writable workspace and wait for its initial peer synchronization to finish.
    pub async fn import_writable_workspace_synced(&self, ticket: &str) -> Result<IrohWorkspace> {
        validate_writable_ticket(ticket)?;
        let ticket = DocTicket::from_str(ticket).context("parse workspace ticket")?;
        let (doc, mut events) = self
            .docs
            .import_and_subscribe(ticket)
            .await
            .context("import writable workspace ticket")?;
        let wait_for_sync = async {
            while let Some(event) = events.next().await {
                if let LiveEvent::SyncFinished(event) =
                    event.context("read workspace sync event")?
                {
                    event
                        .result
                        .map_err(|error| anyhow::anyhow!(error))
                        .context("initial workspace synchronization")?;
                    return Ok(());
                }
            }
            bail!("workspace synchronization event stream ended before initial sync");
        };
        tokio::time::timeout(Duration::from_secs(30), wait_for_sync)
            .await
            .context("timed out waiting for initial workspace synchronization")??;
        Ok(self.workspace(doc))
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.router.shutdown().await.context("shutdown Iroh router")
    }

    fn workspace(&self, doc: Doc) -> IrohWorkspace {
        let endpoint_id = self.endpoint_id();
        spawn_auto_sync(&doc, endpoint_id);
        IrohWorkspace {
            doc,
            blobs: self.blobs.clone(),
            author: self.author,
            endpoint_id,
        }
    }
}

fn spawn_auto_sync(doc: &Doc, local_endpoint_id: EndpointId) {
    let doc = doc.clone();
    tokio::spawn(async move {
        let Ok(events) = doc.subscribe().await else {
            return;
        };
        futures_lite::pin!(events);
        let mut known_peers = std::collections::HashSet::new();
        while let Some(event) = events.next().await {
            let Ok(event) = event else {
                break;
            };
            match event {
                LiveEvent::NeighborUp(node_id) if node_id != local_endpoint_id => {
                    known_peers.insert(node_id);
                    let _ = doc.start_sync(vec![iroh::EndpointAddr::new(node_id)]).await;
                }
                LiveEvent::InsertRemote { from, .. } if from != local_endpoint_id => {
                    known_peers.insert(from);
                    let peers = known_peers
                        .iter()
                        .copied()
                        .map(iroh::EndpointAddr::new)
                        .collect::<Vec<_>>();
                    let _ = doc.start_sync(peers).await;
                }
                LiveEvent::SyncFinished(event) if event.peer != local_endpoint_id => {
                    known_peers.insert(event.peer);
                }
                _ => {}
            }
        }
    });
}

#[derive(Clone, Debug)]
pub struct IrohWorkspace {
    doc: Doc,
    blobs: BlobStore,
    author: AuthorId,
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
    pub fn id(&self) -> NamespaceId {
        self.doc.id()
    }

    #[must_use]
    pub fn author_id(&self) -> AuthorId {
        self.author
    }

    pub async fn share(&self, writable: bool) -> Result<String> {
        let mode = if writable {
            ShareMode::Write
        } else {
            ShareMode::Read
        };
        let ticket = self
            .doc
            .share(mode, AddrInfoOptions::RelayAndAddresses)
            .await
            .context("create workspace ticket")?;
        Ok(ticket.to_string())
    }

    pub async fn put(&self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Result<String> {
        let hash = self
            .doc
            .set_bytes(self.author, key.into(), value.into())
            .await
            .context("write workspace entry")?;
        Ok(hash.to_string())
    }

    /// Store bytes as a durable blob and publish a Docs hash reference under `key`.
    pub async fn put_blob(
        &self,
        key: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
    ) -> Result<String> {
        let key = key.into();
        let value = value.into();
        let size = u64::try_from(value.len()).context("blob size exceeds u64")?;
        let tag = format!("xo/{}/{}", self.id(), blake3::hash(&key).to_hex());
        let hash_and_format = self
            .blobs
            .blobs()
            .add_bytes(value)
            .with_named_tag(tag)
            .await
            .context("store workspace blob")?;
        self.doc
            .set_hash(self.author, key, hash_and_format.hash, size)
            .await
            .context("publish workspace blob reference")?;
        Ok(hash_and_format.hash.to_string())
    }

    pub async fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<Vec<u8>>> {
        Ok(self.get_signed(key).await?.map(|entry| entry.value))
    }

    /// Return a latest value together with its verified Iroh Docs signing author.
    pub async fn get_signed(&self, key: impl AsRef<[u8]>) -> Result<Option<SignedWorkspaceValue>> {
        let query = Query::single_latest_per_key().key_exact(key).build();
        let Some(entry) = self
            .doc
            .get_one(query)
            .await
            .context("query workspace entry")?
        else {
            return Ok(None);
        };
        let Ok(bytes) = self.blobs.blobs().get_bytes(entry.content_hash()).await else {
            return Ok(None);
        };
        Ok(Some(SignedWorkspaceValue {
            key: entry.key().to_vec(),
            value: bytes.to_vec(),
            author: entry.author().to_string(),
        }))
    }

    /// Return the latest value for every key below `prefix`, ordered by key.
    pub async fn list(&self, prefix: impl AsRef<[u8]>) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        Ok(self
            .list_signed(prefix)
            .await?
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect())
    }

    /// Return latest values together with their verified Iroh Docs signing authors.
    pub async fn list_signed(&self, prefix: impl AsRef<[u8]>) -> Result<Vec<SignedWorkspaceValue>> {
        let query = Query::single_latest_per_key().key_prefix(prefix).build();
        let entries = self
            .doc
            .get_many(query)
            .await
            .context("query workspace entries")?;
        futures_lite::pin!(entries);
        let mut values = Vec::new();
        while let Some(entry) = entries.next().await {
            let entry = entry.context("read workspace entry")?;
            let Ok(bytes) = self.blobs.blobs().get_bytes(entry.content_hash()).await else {
                let _ = self.doc.start_sync(vec![]).await;
                continue;
            };
            values.push(SignedWorkspaceValue {
                key: entry.key().to_vec(),
                value: bytes.to_vec(),
                author: entry.author().to_string(),
            });
        }
        values.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(values)
    }

    /// Subscribe to local and replicated document changes.
    pub async fn subscribe(
        &self,
    ) -> Result<impl futures_lite::Stream<Item = Result<WorkspaceEvent>> + Send + Unpin + 'static>
    {
        use iroh_docs::sync::ContentStatus;

        let events = self
            .doc
            .subscribe()
            .await
            .context("subscribe to workspace")?;
        let doc = self.doc.clone();
        Ok(events.map(move |event| {
            let event = event.context("read workspace event")?;
            let event = match event {
                LiveEvent::InsertLocal { .. }
                | LiveEvent::InsertRemote {
                    content_status: ContentStatus::Complete,
                    ..
                }
                | LiveEvent::ContentReady { .. }
                | LiveEvent::PendingContentReady => WorkspaceEvent::ContentChanged,
                LiveEvent::NeighborUp(node_id) => {
                    let doc = doc.clone();
                    tokio::spawn(async move {
                        let _ = doc.start_sync(vec![iroh::EndpointAddr::new(node_id)]).await;
                    });
                    WorkspaceEvent::StatusChanged
                }
                LiveEvent::InsertRemote { .. }
                | LiveEvent::NeighborDown(_)
                | LiveEvent::SyncFinished(_) => WorkspaceEvent::StatusChanged,
            };
            Ok(event)
        }))
    }

    pub async fn start_sync(&self, ticket: &str) -> Result<()> {
        let ticket = DocTicket::from_str(ticket).context("parse workspace ticket")?;
        if ticket.capability.id() != self.id() {
            bail!("ticket belongs to a different workspace");
        }
        let remote_nodes = ticket
            .nodes
            .into_iter()
            .filter(|node| node.id != self.endpoint_id)
            .collect();
        self.doc
            .start_sync(remote_nodes)
            .await
            .context("start workspace synchronization")
    }

    /// Resume synchronization using peers persisted by Iroh Docs.
    pub async fn resume_sync(&self) -> Result<()> {
        self.doc
            .start_sync(vec![])
            .await
            .context("resume workspace synchronization")
    }
}

fn load_or_create_secret_key(path: &Path) -> Result<SecretKey> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("endpoint key must contain exactly 32 bytes"))?;
            Ok(SecretKey::from_bytes(&bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let secret = SecretKey::generate();
            write_secret_file(path, &secret.to_bytes())?;
            Ok(secret)
        }
        Err(error) => Err(error).with_context(|| format!("read endpoint key {}", path.display())),
    }
}

#[cfg(unix)]
fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create endpoint key {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use iroh_blobs::Hash;
    use iroh_blobs::api::blobs::BlobStatus;
    use iroh_blobs::protocol::{ChunkRanges, ChunkRangesExt};

    use super::*;

    async fn wait_for_value(workspace: &IrohWorkspace, key: &str) -> Result<Vec<u8>> {
        let mut last_error = None;
        for _ in 0..300 {
            match workspace.get(key).await {
                Ok(Some(value)) => return Ok(value),
                Ok(None) => {}
                Err(error) => last_error = Some(error),
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        if let Some(error) = last_error {
            return Err(error).with_context(|| format!("wait for workspace value {key}"));
        }
        bail!("workspace value {key} did not replicate");
    }

    #[tokio::test]
    async fn two_peers_sync_and_second_peer_reconnects_after_restart() -> Result<()> {
        let _guard = IROH_TEST_LOCK.lock().await;
        let first_dir = tempfile::tempdir()?;
        let second_dir = tempfile::tempdir()?;

        let first = IrohNode::persistent(first_dir.path()).await?;
        let workspace = first.create_workspace().await?;
        let workspace_id = workspace.id();
        workspace.put("note/test/revision/one", "hello").await?;
        let ticket = workspace.share(true).await?;

        let second = IrohNode::persistent(second_dir.path()).await?;
        let second_endpoint = second.endpoint_id();
        let imported = second.import_workspace(&ticket).await?;
        assert_eq!(imported.id(), workspace_id);

        let replicated = wait_for_value(&imported, "note/test/revision/one").await?;
        assert_eq!(replicated, b"hello");

        second.shutdown().await?;
        drop(imported);
        drop(second);

        let restarted = IrohNode::persistent(second_dir.path()).await?;
        assert_eq!(restarted.endpoint_id(), second_endpoint);
        let reopened = restarted
            .open_workspace(workspace_id)
            .await?
            .context("workspace missing after restart")?;
        assert_eq!(
            reopened.get("note/test/revision/one").await?,
            Some(b"hello".to_vec())
        );
        reopened.start_sync(&ticket).await?;
        workspace
            .put("note/test/revision/two", "after restart")
            .await?;
        let resumed = wait_for_value(&reopened, "note/test/revision/two").await?;
        assert_eq!(resumed, b"after restart");

        restarted.shutdown().await?;
        first.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn read_only_ticket_cannot_write() -> Result<()> {
        let _guard = IROH_TEST_LOCK.lock().await;
        let owner_dir = tempfile::tempdir()?;
        let reader_dir = tempfile::tempdir()?;
        let owner = IrohNode::persistent(owner_dir.path()).await?;
        let workspace = owner.create_workspace().await?;
        workspace.put("note/test/revision/one", "owner").await?;
        let ticket = workspace.share(false).await?;

        let reader = IrohNode::persistent(reader_dir.path()).await?;
        let imported = reader.import_workspace(&ticket).await?;
        assert!(
            imported
                .put("note/test/revision/two", "reader")
                .await
                .is_err()
        );

        reader.shutdown().await?;
        owner.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn partial_blob_survives_restart_and_resumes_during_sync() -> Result<()> {
        let _guard = IROH_TEST_LOCK.lock().await;
        let source_dir = tempfile::tempdir()?;
        let receiver_dir = tempfile::tempdir()?;
        let source = IrohNode::persistent(source_dir.path()).await?;
        let workspace = source.create_workspace().await?;
        let bytes = (0_u8..=250)
            .cycle()
            .take(4 * 1024 * 1024)
            .collect::<Vec<_>>();
        let hash = Hash::from_str(
            &workspace
                .put_blob("asset-blob/large", bytes.clone())
                .await?,
        )?;
        let ticket = workspace.share(true).await?;

        // Persist only verified beginning and ending ranges, equivalent to a transfer being
        // interrupted after some chunks reached the receiver.
        let ranges = ChunkRanges::bytes(..256 * 1024) | ChunkRanges::last_chunk();
        let partial_bao = workspace
            .blobs
            .blobs()
            .export_bao(hash, ranges.clone())
            .bao_to_vec()
            .await?;
        let receiver = IrohNode::persistent(receiver_dir.path()).await?;
        receiver
            .blobs
            .blobs()
            .import_bao_bytes(hash, ranges, partial_bao)
            .await?;
        assert!(matches!(
            receiver.blobs.blobs().status(hash).await?,
            BlobStatus::Partial { .. }
        ));
        receiver.shutdown().await?;
        drop(receiver);

        let receiver = IrohNode::persistent(receiver_dir.path()).await?;
        assert!(matches!(
            receiver.blobs.blobs().status(hash).await?,
            BlobStatus::Partial { .. }
        ));
        let imported = receiver.import_workspace(&ticket).await?;
        let completed_bytes = wait_for_value(&imported, "asset-blob/large").await?;
        assert_eq!(completed_bytes, bytes);
        assert_eq!(
            receiver.blobs.blobs().status(hash).await?,
            BlobStatus::Complete {
                size: bytes.len() as u64
            }
        );
        receiver.shutdown().await?;
        source.shutdown().await?;
        Ok(())
    }
}

#[cfg(not(unix))]
fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create endpoint key {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
