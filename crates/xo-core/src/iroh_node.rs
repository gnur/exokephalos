//! Persistent native Iroh protocol composition.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use futures_lite::StreamExt;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointId, SecretKey};
use iroh_blobs::api::Store as BlobStore;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::{ALPN as BLOBS_ALPN, BlobsProtocol};
use iroh_docs::api::protocol::{AddrInfoOptions, ShareMode};
use iroh_docs::api::{Doc, DocsApi};
use iroh_docs::protocol::Docs;
use iroh_docs::store::Query;
use iroh_docs::{ALPN as DOCS_ALPN, AuthorId, DocTicket, NamespaceId};
use iroh_gossip::ALPN as GOSSIP_ALPN;
use iroh_gossip::net::Gossip;

const ENDPOINT_KEY_FILE: &str = "endpoint.key";

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
        let state_dir = state_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&state_dir)
            .with_context(|| format!("create state directory {}", state_dir.display()))?;
        let secret_key = load_or_create_secret_key(&state_dir.join(ENDPOINT_KEY_FILE))?;
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret_key)
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

    pub async fn import_workspace(&self, ticket: &str) -> Result<IrohWorkspace> {
        let ticket = DocTicket::from_str(ticket).context("parse workspace ticket")?;
        let doc = self
            .docs
            .import(ticket)
            .await
            .context("import workspace ticket")?;
        Ok(self.workspace(doc))
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.router.shutdown().await.context("shutdown Iroh router")
    }

    fn workspace(&self, doc: Doc) -> IrohWorkspace {
        IrohWorkspace {
            doc,
            blobs: self.blobs.clone(),
            author: self.author,
        }
    }
}

#[derive(Clone, Debug)]
pub struct IrohWorkspace {
    doc: Doc,
    blobs: BlobStore,
    author: AuthorId,
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

    pub async fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<Vec<u8>>> {
        let query = Query::single_latest_per_key().key_exact(key).build();
        let Some(entry) = self
            .doc
            .get_one(query)
            .await
            .context("query workspace entry")?
        else {
            return Ok(None);
        };
        let bytes = self
            .blobs
            .blobs()
            .get_bytes(entry.content_hash())
            .await
            .context("read workspace entry blob")?;
        Ok(Some(bytes.to_vec()))
    }

    /// Return the latest value for every key below `prefix`, ordered by key.
    pub async fn list(&self, prefix: impl AsRef<[u8]>) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
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
            let bytes = self
                .blobs
                .blobs()
                .get_bytes(entry.content_hash())
                .await
                .context("read workspace entry blob")?;
            values.push((entry.key().to_vec(), bytes.to_vec()));
        }
        values.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(values)
    }

    pub async fn start_sync(&self, ticket: &str) -> Result<()> {
        let ticket = DocTicket::from_str(ticket).context("parse workspace ticket")?;
        if ticket.capability.id() != self.id() {
            bail!("ticket belongs to a different workspace");
        }
        self.doc
            .start_sync(ticket.nodes)
            .await
            .context("start workspace synchronization")
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

    use super::*;

    #[tokio::test]
    async fn two_peers_sync_and_second_peer_survives_restart() -> Result<()> {
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

        let mut replicated = None;
        for _ in 0..100 {
            if let Some(value) = imported.get("note/test/revision/one").await? {
                replicated = Some(value);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(replicated.as_deref(), Some(b"hello".as_slice()));

        second.shutdown().await?;
        first.shutdown().await?;
        drop(imported);
        drop(workspace);
        drop(second);
        drop(first);

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
        restarted.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn read_only_ticket_cannot_write() -> Result<()> {
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
