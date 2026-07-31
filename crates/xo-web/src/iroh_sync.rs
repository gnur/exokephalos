use std::str::FromStr;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_lite::StreamExt;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, SecretKey};
use iroh_blobs::api::Store as BlobStore;
use iroh_blobs::store::mem::MemStore;
use iroh_blobs::{ALPN as BLOBS_ALPN, BlobsProtocol};
use iroh_docs::api::Doc;
use iroh_docs::api::protocol::{AddrInfoOptions, ShareMode};
use iroh_docs::engine::LiveEvent;
use iroh_docs::protocol::Docs;
use iroh_docs::store::Query;
use iroh_docs::{ALPN as DOCS_ALPN, Author, AuthorId, DocTicket};
use iroh_gossip::ALPN as GOSSIP_ALPN;
use iroh_gossip::net::Gossip;
use js_sys::Uint8Array;
use n0_future::time::{self, Duration};
use serde::Serialize;
use wasm_bindgen::JsError;
use wasm_bindgen::prelude::*;

const SYNC_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ENTRY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceOutcome {
    workspace_id: String,
    ticket: String,
    sync_error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncStatus {
    endpoint_id: String,
    workspace_id: Option<String>,
    author_id: String,
    peers: usize,
    writable: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentEntry {
    key: String,
    key_base64: String,
    value: Option<String>,
    value_base64: String,
    author: String,
    content_hash: String,
    content_len: u64,
}

/// Relay-only Iroh Docs node intended to live for the lifetime of a Web Worker.
#[wasm_bindgen]
pub struct IrohDocNode {
    router: Router,
    docs: Docs,
    blobs: BlobStore,
    author: AuthorId,
    document: Option<Doc>,
    ticket: Option<DocTicket>,
}

#[wasm_bindgen]
impl IrohDocNode {
    /// Start Docs, Blobs, and Gossip using persisted endpoint and author keys.
    #[wasm_bindgen(js_name = spawn)]
    pub async fn spawn(
        endpoint_secret: Uint8Array,
        author_secret: Uint8Array,
    ) -> Result<IrohDocNode, JsError> {
        let endpoint_secret = fixed_secret(&endpoint_secret, "endpoint")?;
        let author_secret = fixed_secret(&author_secret, "author")?;
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(SecretKey::from_bytes(&endpoint_secret))
            .bind()
            .await
            .map_err(js_error)?;
        let mem_store = MemStore::default();
        let blobs = mem_store.as_ref().clone();
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let docs = Docs::memory()
            .spawn(endpoint.clone(), blobs.clone(), gossip.clone())
            .await
            .map_err(js_error)?;
        let author = Author::from_bytes(&author_secret);
        let author_id = author.id();
        docs.api().author_import(author).await.map_err(js_error)?;
        docs.api()
            .author_set_default(author_id)
            .await
            .map_err(js_error)?;
        let router = Router::builder(endpoint)
            .accept(BLOBS_ALPN, BlobsProtocol::new(&blobs, None))
            .accept(GOSSIP_ALPN, gossip)
            .accept(DOCS_ALPN, docs.clone())
            .spawn();
        Ok(Self {
            router,
            docs,
            blobs,
            author: author_id,
            document: None,
            ticket: None,
        })
    }

    /// Create a writable workspace and return its share ticket.
    #[wasm_bindgen(js_name = createWorkspace)]
    pub async fn create_workspace(&mut self) -> Result<String, JsError> {
        let document = self.docs.create().await.map_err(js_error)?;
        self.document = Some(document);
        let ticket = self.share_ticket().await?;
        self.ticket = Some(DocTicket::from_str(&ticket).map_err(js_error)?);
        json(&WorkspaceOutcome {
            workspace_id: self.document().map_err(js_error)?.id().to_string(),
            ticket,
            sync_error: None,
        })
        .map_err(js_error)
    }

    /// Import a ticket, begin live synchronization, and wait for initial content.
    /// The document remains open when the peer is temporarily unavailable.
    #[wasm_bindgen(js_name = joinWorkspace)]
    pub async fn join_workspace(&mut self, ticket: String) -> Result<String, JsError> {
        let ticket = DocTicket::from_str(ticket.trim()).map_err(js_error)?;
        if ticket.capability.secret_key().is_err() {
            return Err(JsError::new("xo-web requires a writable workspace ticket"));
        }
        let document = self
            .docs
            .api()
            .import_namespace(ticket.capability.clone())
            .await
            .map_err(js_error)?;
        let mut events = document.subscribe().await.map_err(js_error)?;
        let sync_start = document.start_sync(ticket.nodes.clone()).await;
        self.document = Some(document);
        self.ticket = Some(ticket.clone());
        let sync_error = match sync_start {
            Ok(()) => wait_for_initial_sync(&mut events)
                .await
                .err()
                .map(|error| error.to_string()),
            Err(error) => Some(error.to_string()),
        };
        json(&WorkspaceOutcome {
            workspace_id: ticket.capability.id().to_string(),
            ticket: ticket.to_string(),
            sync_error,
        })
        .map_err(js_error)
    }

    /// Retry live synchronization using the peers retained in the ticket.
    #[wasm_bindgen(js_name = refreshSync)]
    pub async fn refresh_sync(&self) -> Result<(), JsError> {
        let document = self.document().map_err(js_error)?;
        let ticket = self
            .ticket
            .as_ref()
            .context("no workspace ticket is loaded")
            .map_err(js_error)?;
        let mut events = document.subscribe().await.map_err(js_error)?;
        document
            .start_sync(ticket.nodes.clone())
            .await
            .map_err(js_error)?;
        wait_for_initial_sync(&mut events).await.map_err(js_error)
    }

    /// Publish UTF-8 bytes under a document key.
    #[wasm_bindgen(js_name = putText)]
    pub async fn put_text(&self, key: String, value: String) -> Result<String, JsError> {
        if key.is_empty() {
            return Err(JsError::new("document key is required"));
        }
        if value.len() > MAX_ENTRY_BYTES {
            return Err(JsError::new("document value exceeds 8 MiB"));
        }
        let hash = self
            .document()
            .map_err(js_error)?
            .set_bytes(self.author, key.into_bytes(), value.into_bytes())
            .await
            .map_err(js_error)?;
        Ok(hash.to_string())
    }

    /// Return the latest value for each document key, including raw base64.
    #[wasm_bindgen(js_name = entriesJson)]
    pub async fn entries_json(&self) -> Result<String, JsError> {
        let document = self.document().map_err(js_error)?;
        let entries = document
            .get_many(Query::single_latest_per_key().build())
            .await
            .map_err(js_error)?;
        futures_lite::pin!(entries);
        let mut output = Vec::new();
        while let Some(entry) = entries.next().await {
            let entry = entry.map_err(js_error)?;
            let bytes = self
                .blobs
                .blobs()
                .get_bytes(entry.content_hash())
                .await
                .map_err(js_error)?;
            let key = entry.key();
            output.push(DocumentEntry {
                key: String::from_utf8_lossy(key).into_owned(),
                key_base64: BASE64.encode(key),
                value: std::str::from_utf8(&bytes).ok().map(str::to_owned),
                value_base64: BASE64.encode(&bytes),
                author: entry.author().to_string(),
                content_hash: entry.content_hash().to_string(),
                content_len: entry.content_len(),
            });
        }
        json(&output).map_err(js_error)
    }

    /// Return endpoint, workspace, author, and active peer diagnostics.
    #[wasm_bindgen(js_name = statusJson)]
    pub async fn status_json(&self) -> Result<String, JsError> {
        let (workspace_id, peers, writable) = if let Some(document) = &self.document {
            let peers = document
                .get_sync_peers()
                .await
                .map_err(js_error)?
                .map_or(0, |peers| peers.len());
            let writable = self
                .ticket
                .as_ref()
                .is_some_and(|ticket| ticket.capability.secret_key().is_ok());
            (Some(document.id().to_string()), peers, writable)
        } else {
            (None, 0, false)
        };
        json(&SyncStatus {
            endpoint_id: self.router.endpoint().id().to_string(),
            workspace_id,
            author_id: self.author.to_string(),
            peers,
            writable,
        })
        .map_err(js_error)
    }

    /// Create a fresh ticket containing the endpoint's current relay address.
    #[wasm_bindgen(js_name = shareTicket)]
    pub async fn share_ticket(&self) -> Result<String, JsError> {
        time::timeout(SYNC_TIMEOUT, self.router.endpoint().online())
            .await
            .map_err(js_error)?;
        let ticket = self
            .document()
            .map_err(js_error)?
            .share(ShareMode::Write, AddrInfoOptions::RelayAndAddresses)
            .await
            .map_err(js_error)?;
        Ok(ticket.to_string())
    }
}

impl IrohDocNode {
    fn document(&self) -> Result<&Doc> {
        self.document.as_ref().context("no workspace is open")
    }
}

async fn wait_for_initial_sync<S>(events: &mut S) -> Result<()>
where
    S: futures_lite::Stream<Item = Result<LiveEvent>> + Unpin,
{
    time::timeout(SYNC_TIMEOUT, async {
        while let Some(event) = events.next().await {
            match event? {
                LiveEvent::SyncFinished(event) => {
                    if let Err(error) = event.result {
                        bail!("initial document sync failed: {error}");
                    }
                }
                LiveEvent::PendingContentReady => return Ok(()),
                _ => {}
            }
        }
        bail!("document event stream ended before initial sync completed")
    })
    .await
    .context("initial document sync timed out")?
}

fn fixed_secret(value: &Uint8Array, name: &str) -> Result<[u8; 32], JsError> {
    if value.length() != 32 {
        return Err(JsError::new(&format!("{name} secret must be 32 bytes")));
    }
    let mut output = [0; 32];
    value.copy_to(&mut output);
    Ok(output)
}

fn json(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).context("serialize browser Iroh result")
}

fn js_error(error: impl Into<anyhow::Error>) -> JsError {
    JsError::new(&format!("{:#}", error.into()))
}
