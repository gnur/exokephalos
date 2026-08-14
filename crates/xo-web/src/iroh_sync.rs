use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result, bail};
use async_lock::{Mutex, RwLock};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use iroh::address_lookup::{
    AddressLookup, AddressLookupBuilder, AddressLookupBuilderError, EndpointInfo,
    Error as AddressLookupError, Item as AddressLookupItem, PkarrPublisher, PkarrResolver,
};
use iroh::endpoint::presets;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr};
use iroh_gossip::api::{Event as GossipEvent, GossipSender};
use iroh_gossip::net::Gossip;
use iroh_gossip::{ALPN as GOSSIP_ALPN, TopicId};
use js_sys::Uint8Array;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsError;
use wasm_bindgen::prelude::*;
use xo_core::authenticated_change::SignedAutomergeChange;
use xo_core::automerge_store::AutomergeRecordStore;
use xo_core::membership::{
    MemberStatus, MembershipEvent, MembershipIdentity, MembershipRegistry, PeerId,
    SignedMembershipEvent, public_key_fingerprint,
};
use xo_core::peer_protocol::{
    AUTOMERGE_ALPN, AuthHello, GossipAnnouncement, JOIN_ALPN, JoinRequest, JoinResponse,
    MAX_PROTOCOL_FRAME, PROTOCOL_VERSION, WorkspaceInvitation, decode, encode,
};
use xo_core::{CURRENT_SCHEMA, DeviceRecord, Hlc, WorkspaceId};

const MAX_ENTRY_BYTES: usize = 8 * 1024 * 1024;

type WorkspaceMap = Arc<RwLock<BTreeMap<String, Arc<BrowserWorkspace>>>>;

#[derive(Debug)]
struct BrowserWorkspace {
    id: String,
    document: Mutex<AutomergeRecordStore>,
    registry: RwLock<MembershipRegistry>,
    pending: RwLock<BTreeMap<String, JoinRequest>>,
    signed: RwLock<BTreeMap<String, SignedAutomergeChange>>,
    peers: RwLock<BTreeMap<iroh::EndpointId, EndpointAddr>>,
    genesis_fingerprint: String,
    gossip_topic: RwLock<[u8; 32]>,
    base_gossip_topic: [u8; 32],
    membership_epoch: AtomicU64,
    gossip_sender: Mutex<Option<GossipSender>>,
    identity: Arc<MembershipIdentity>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SignedSyncPayload {
    workspace_id: String,
    changes: Vec<SignedAutomergeChange>,
}

#[derive(Serialize, Deserialize)]
struct BrowserReplica {
    workspace_id: String,
    snapshot: Vec<u8>,
    signed_changes: Vec<SignedAutomergeChange>,
    pending_requests: Vec<JoinRequest>,
}

impl BrowserWorkspace {
    async fn refresh_registry(&self) -> Result<()> {
        let encoded = self.document.lock().await.scan("membership/event/")?;
        let events = encoded
            .values()
            .map(|bytes| decode::<SignedMembershipEvent>(bytes).map_err(anyhow::Error::from))
            .collect::<Result<Vec<_>>>()?;
        let mut removals = events
            .iter()
            .filter(|event| matches!(event.payload, MembershipEvent::PeerRemoved { .. }))
            .map(|event| (event.issued_at.clone(), event.event_id.clone()))
            .collect::<Vec<_>>();
        removals.sort();
        let mut remaining = events;
        let mut registry = MembershipRegistry::default();
        while !remaining.is_empty() {
            let previous = remaining.len();
            remaining.retain(|event| registry.apply(event).is_err());
            if remaining.len() == previous {
                bail!("membership event graph is invalid");
            }
        }
        *self.registry.write().await = registry;
        let removal_count = removals.len();
        let mut topic = self.base_gossip_topic;
        for (_, event_id) in removals {
            let mut material = topic.to_vec();
            material.extend_from_slice(event_id.as_bytes());
            topic = *blake3::hash(&material).as_bytes();
        }
        *self.gossip_topic.write().await = topic;
        self.membership_epoch
            .store(u64::try_from(removal_count)?, Ordering::Release);
        Ok(())
    }

    async fn sign_local(&self) -> Result<()> {
        let changes = self.document.lock().await.clone().all_changes();
        let mut signed = self.signed.write().await;
        for change in changes {
            if change.actor_id().to_bytes() == self.identity.public_key()
                && !signed.contains_key(&change.hash().to_string())
            {
                let envelope = SignedAutomergeChange::create(&self.id, &self.identity, &change)?;
                signed.insert(envelope.change_hash.clone(), envelope);
            }
        }
        Ok(())
    }

    async fn payload(&self) -> Result<SignedSyncPayload> {
        let changes = self.document.lock().await.clone().all_changes();
        let signed = self.signed.read().await;
        Ok(SignedSyncPayload {
            workspace_id: self.id.clone(),
            changes: changes
                .iter()
                .map(|change| {
                    signed
                        .get(&change.hash().to_string())
                        .cloned()
                        .context("Automerge change has no signature")
                })
                .collect::<Result<_>>()?,
        })
    }

    async fn apply(&self, payload: SignedSyncPayload) -> Result<()> {
        if payload.workspace_id != self.id {
            bail!("sync payload belongs to a different workspace");
        }
        for envelope in payload.changes {
            if self.signed.read().await.contains_key(&envelope.change_hash) {
                continue;
            }
            let change = envelope.verify()?;
            let fingerprint = public_key_fingerprint(&envelope.public_key);
            let authorized = {
                let registry = self.registry.read().await;
                registry.member(&fingerprint).is_some_and(|member| {
                    member.status == MemberStatus::Active
                        || (member.status == MemberStatus::Removed
                            && member
                                .accepted_actor_sequence
                                .is_some_and(|cutoff| envelope.sequence <= cutoff))
                }) || (registry.members().next().is_none()
                    && fingerprint == self.genesis_fingerprint)
            };
            if !authorized {
                bail!("Automerge change actor is not authorized");
            }
            self.document.lock().await.apply_changes([change])?;
            self.signed
                .write()
                .await
                .insert(envelope.change_hash.clone(), envelope);
            self.refresh_registry().await?;
        }
        Ok(())
    }
}

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
    pending_approval: bool,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedEntry {
    key_base64: String,
    value_base64: String,
    author: String,
    #[serde(default)]
    pending: bool,
}

/// Relay-only browser Automerge node using authenticated Iroh QUIC streams.
#[wasm_bindgen]
pub struct IrohDocNode {
    router: Router,
    identity: Arc<MembershipIdentity>,
    workspaces: WorkspaceMap,
    workspace: Option<Arc<BrowserWorkspace>>,
    invitation: Option<WorkspaceInvitation>,
    relay_map: RelayMap,
    gossip: Gossip,
    pending_approval: bool,
}

#[wasm_bindgen]
impl IrohDocNode {
    #[wasm_bindgen(js_name = spawn)]
    pub async fn spawn(
        endpoint_secret: Uint8Array,
        author_secret: Uint8Array,
        peer_id: String,
    ) -> Result<IrohDocNode, JsError> {
        let endpoint_secret = fixed_secret(&endpoint_secret, "endpoint")?;
        let author_secret = fixed_secret(&author_secret, "membership")?;
        let identity = Arc::new(MembershipIdentity::from_secret_bytes(
            PeerId::parse(peer_id).map_err(js_error)?,
            &author_secret,
        ));
        let relay_map = browser_relay_map().map_err(js_error)?;
        let endpoint = Endpoint::builder(presets::Minimal)
            .address_lookup(PkarrPublisher::n0_dns())
            .address_lookup(NormalizedPkarrResolverBuilder(PkarrResolver::n0_dns()))
            .relay_mode(RelayMode::Custom(relay_map.clone()))
            .secret_key(SecretKey::from_bytes(&endpoint_secret))
            .bind()
            .await
            .map_err(js_error)?;
        let workspaces = Arc::new(RwLock::new(BTreeMap::new()));
        let endpoint_id = endpoint.id();
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let router = Router::builder(endpoint)
            .accept(
                JOIN_ALPN,
                BrowserJoin {
                    workspaces: workspaces.clone(),
                    identity: identity.clone(),
                },
            )
            .accept(GOSSIP_ALPN, gossip.clone())
            .accept(
                AUTOMERGE_ALPN,
                BrowserSync {
                    workspaces: workspaces.clone(),
                    identity: identity.clone(),
                    endpoint_id,
                },
            )
            .spawn();
        Ok(Self {
            router,
            identity,
            workspaces,
            workspace: None,
            invitation: None,
            relay_map,
            gossip,
            pending_approval: false,
        })
    }

    #[wasm_bindgen(js_name = createWorkspace)]
    pub async fn create_workspace(&mut self) -> Result<String, JsError> {
        let id = valid_workspace_id();
        let mut document =
            AutomergeRecordStore::create(&id, &self.identity.public_key()).map_err(js_error)?;
        let genesis = SignedMembershipEvent::create(
            &self.identity,
            WorkspaceId::new(id.clone()),
            now_hlc(&self.identity).map_err(js_error)?,
            MembershipEvent::Genesis {
                peer_id: self.identity.peer_id().clone(),
                public_key: self.identity.public_key(),
                endpoint_id: self.router.endpoint().id().to_string(),
            },
        )
        .map_err(js_error)?;
        document
            .put(
                &format!("membership/event/{}", genesis.event_id),
                encode(&genesis).map_err(js_error)?,
            )
            .map_err(js_error)?;
        let gossip_topic: [u8; 32] = rand::random();
        let workspace = Arc::new(BrowserWorkspace {
            id: id.clone(),
            document: Mutex::new(document),
            registry: RwLock::new(MembershipRegistry::default()),
            pending: RwLock::new(BTreeMap::new()),
            signed: RwLock::new(BTreeMap::new()),
            peers: RwLock::new(BTreeMap::new()),
            genesis_fingerprint: self.identity.fingerprint(),
            gossip_topic: RwLock::new(gossip_topic),
            base_gossip_topic: gossip_topic,
            membership_epoch: AtomicU64::new(0),
            gossip_sender: Mutex::new(None),
            identity: self.identity.clone(),
        });
        workspace.sign_local().await.map_err(js_error)?;
        workspace.refresh_registry().await.map_err(js_error)?;
        self.workspaces
            .write()
            .await
            .insert(id.clone(), workspace.clone());
        self.workspace = Some(workspace.clone());
        self.pending_approval = false;
        start_browser_gossip(
            self.gossip.clone(),
            self.router.endpoint().clone(),
            self.identity.clone(),
            workspace,
        )
        .await
        .map_err(js_error)?;
        self.register_browser_device().await.map_err(js_error)?;
        let ticket = self.share_ticket().await?;
        json(&WorkspaceOutcome {
            workspace_id: id,
            ticket,
            sync_error: None,
        })
        .map_err(js_error)
    }

    #[wasm_bindgen(js_name = joinWorkspace)]
    pub async fn join_workspace(&mut self, ticket: String) -> Result<String, JsError> {
        let mut invitation = WorkspaceInvitation::decode(ticket.trim()).map_err(js_error)?;
        normalize_browser_nodes(&mut invitation.bootstrap_peers).map_err(js_error)?;
        self.relay_map
            .extend(&relay_map_for_nodes(&invitation.bootstrap_peers));
        let response = self.request_join(&invitation).await.map_err(js_error)?;
        self.invitation = Some(invitation.clone());
        if matches!(
            response,
            JoinResponse::Pending | JoinResponse::Approved { .. }
        ) {
            // Admission is always confirmed by a follow-up request. This keeps
            // the browser's polling path deterministic even when the active
            // peer records the approval during the first round trip.
            self.pending_approval = true;
            return json(&WorkspaceOutcome {
                workspace_id: invitation.workspace_id,
                ticket,
                sync_error: Some("workspace membership request is pending approval".into()),
            })
            .map_err(js_error);
        }
        if matches!(response, JoinResponse::Rejected) {
            return Err(JsError::new("workspace membership request was rejected"));
        }
        self.ensure_join_workspace(&invitation)
            .await
            .map_err(js_error)?;
        self.sync_invitation(&invitation).await.map_err(js_error)?;
        self.pending_approval = false;
        self.register_browser_device().await.map_err(js_error)?;
        json(&WorkspaceOutcome {
            workspace_id: invitation.workspace_id,
            ticket,
            sync_error: None,
        })
        .map_err(js_error)
    }

    #[wasm_bindgen(js_name = restoreReplica)]
    pub async fn restore_replica(&mut self, ticket: String, value: String) -> Result<(), JsError> {
        let invitation = WorkspaceInvitation::decode(&ticket).map_err(js_error)?;
        let bytes = BASE64.decode(value).map_err(js_error)?;
        let replica: BrowserReplica = decode(&bytes).map_err(js_error)?;
        if replica.workspace_id != invitation.workspace_id {
            return Err(JsError::new(
                "persisted replica belongs to another workspace",
            ));
        }
        let document = AutomergeRecordStore::load(&replica.snapshot, &self.identity.public_key())
            .map_err(js_error)?;
        let signed = replica
            .signed_changes
            .into_iter()
            .map(|change| {
                Ok((change.change_hash.clone(), {
                    change.verify().map_err(js_error)?;
                    change
                }))
            })
            .collect::<Result<BTreeMap<_, _>, JsError>>()?;
        let pending = replica
            .pending_requests
            .into_iter()
            .map(|request| {
                request.verify().map_err(js_error)?;
                Ok((public_key_fingerprint(&request.public_key), request))
            })
            .collect::<Result<BTreeMap<_, _>, JsError>>()?;
        let workspace = Arc::new(BrowserWorkspace {
            id: invitation.workspace_id.clone(),
            document: Mutex::new(document),
            registry: RwLock::new(MembershipRegistry::default()),
            pending: RwLock::new(pending),
            signed: RwLock::new(signed),
            peers: RwLock::new(
                invitation
                    .bootstrap_peers
                    .iter()
                    .cloned()
                    .map(|p| (p.id, p))
                    .collect(),
            ),
            genesis_fingerprint: invitation.genesis_key_fingerprint.clone(),
            gossip_topic: RwLock::new(invitation.gossip_topic),
            base_gossip_topic: invitation.base_gossip_topic,
            membership_epoch: AtomicU64::new(invitation.membership_epoch),
            gossip_sender: Mutex::new(None),
            identity: self.identity.clone(),
        });
        workspace.refresh_registry().await.map_err(js_error)?;
        self.workspaces
            .write()
            .await
            .insert(workspace.id.clone(), workspace.clone());
        self.workspace = Some(workspace.clone());
        self.invitation = Some(invitation);
        self.pending_approval = false;
        start_browser_gossip(
            self.gossip.clone(),
            self.router.endpoint().clone(),
            self.identity.clone(),
            workspace,
        )
        .await
        .map_err(js_error)?;
        Ok(())
    }

    #[wasm_bindgen(js_name = replicaBase64)]
    pub async fn replica_base64(&self) -> Result<String, JsError> {
        let workspace = self.workspace().map_err(js_error)?;
        let replica = BrowserReplica {
            workspace_id: workspace.id.clone(),
            snapshot: workspace.document.lock().await.clone().save(),
            signed_changes: workspace.signed.read().await.values().cloned().collect(),
            pending_requests: workspace.pending.read().await.values().cloned().collect(),
        };
        Ok(BASE64.encode(encode(&replica).map_err(js_error)?))
    }

    #[wasm_bindgen(js_name = restoreAuthorEntries)]
    pub async fn restore_author_entries(&self, entries_json: String) -> Result<u32, JsError> {
        let entries: Vec<PersistedEntry> = serde_json::from_str(&entries_json).map_err(js_error)?;
        let author = self.identity.fingerprint();
        let mut restored = 0_u32;
        for entry in entries {
            if entry.pending || entry.author != author {
                continue;
            }
            let key = String::from_utf8(BASE64.decode(entry.key_base64).map_err(js_error)?)
                .map_err(js_error)?;
            let value = BASE64.decode(entry.value_base64).map_err(js_error)?;
            self.put_bytes(key, value).await.map_err(js_error)?;
            restored = restored.saturating_add(1);
        }
        Ok(restored)
    }

    #[wasm_bindgen(js_name = refreshSync)]
    pub async fn refresh_sync(&mut self) -> Result<(), JsError> {
        let invitation = self
            .invitation
            .clone()
            .context("no workspace ticket is loaded")
            .map_err(js_error)?;
        if self.pending_approval {
            match self.request_join(&invitation).await.map_err(js_error)? {
                JoinResponse::Pending => {
                    return Err(JsError::new(
                        "workspace membership request is pending approval",
                    ));
                }
                JoinResponse::Rejected => {
                    return Err(JsError::new("workspace membership request was rejected"));
                }
                JoinResponse::Approved { .. } => {
                    self.ensure_join_workspace(&invitation)
                        .await
                        .map_err(js_error)?;
                    self.pending_approval = false;
                }
            }
        }
        self.sync_invitation(&invitation).await.map_err(js_error)?;
        self.register_browser_device().await.map_err(js_error)?;
        Ok(())
    }

    #[wasm_bindgen(js_name = putText)]
    pub async fn put_text(&self, key: String, value: String) -> Result<String, JsError> {
        self.put_bytes(key, value.into_bytes())
            .await
            .map_err(js_error)
    }

    #[wasm_bindgen(js_name = putBase64)]
    pub async fn put_base64(&self, key: String, value: String) -> Result<String, JsError> {
        self.put_bytes(key, BASE64.decode(value).map_err(js_error)?)
            .await
            .map_err(js_error)
    }

    #[wasm_bindgen(js_name = entriesJson)]
    pub async fn entries_json(&self) -> Result<String, JsError> {
        let workspace = self.workspace().map_err(js_error)?;
        let entries = workspace.document.lock().await.scan("").map_err(js_error)?;
        let output = entries
            .into_iter()
            .filter(|(key, _)| !key.starts_with("membership/"))
            .map(|(key, value)| DocumentEntry {
                key_base64: BASE64.encode(key.as_bytes()),
                value: std::str::from_utf8(&value).ok().map(str::to_owned),
                value_base64: BASE64.encode(&value),
                author: record_author(&key, &value).unwrap_or_else(|| self.identity.fingerprint()),
                content_hash: blake3::hash(&value).to_hex().to_string(),
                content_len: value.len() as u64,
                key,
            })
            .collect::<Vec<_>>();
        json(&output).map_err(js_error)
    }

    #[wasm_bindgen(js_name = statusJson)]
    #[allow(clippy::unused_async)]
    pub async fn status_json(&self) -> Result<String, JsError> {
        let (workspace_id, peers, writable) =
            self.workspace
                .as_ref()
                .map_or((None, 0, false), |workspace| {
                    (
                        Some(workspace.id.clone()),
                        workspace.peers.try_read().map_or(0, |p| p.len()),
                        !self.pending_approval,
                    )
                });
        json(&SyncStatus {
            endpoint_id: self.router.endpoint().id().to_string(),
            workspace_id,
            author_id: self.identity.fingerprint(),
            peers,
            writable,
            pending_approval: self.pending_approval,
        })
        .map_err(js_error)
    }

    #[wasm_bindgen(js_name = pendingMembersJson)]
    pub async fn pending_members_json(&self) -> Result<String, JsError> {
        let workspace = self.workspace().map_err(js_error)?;
        let values = workspace
            .pending
            .read()
            .await
            .values()
            .map(|request| {
                serde_json::json!({
                    "peerId": request.peer_id.to_string(),
                    "fingerprint": public_key_fingerprint(&request.public_key),
                    "publicKey": BASE64.encode(request.public_key),
                    "endpointId": request.endpoint_id,
                })
            })
            .collect::<Vec<_>>();
        json(&values).map_err(js_error)
    }

    #[wasm_bindgen(js_name = membersJson)]
    pub async fn members_json(&self) -> Result<String, JsError> {
        let workspace = self.workspace().map_err(js_error)?;
        let values = workspace
            .registry
            .read()
            .await
            .members()
            .map(|member| {
                serde_json::json!({
                    "peerId": member.peer_id.to_string(),
                    "fingerprint": public_key_fingerprint(&member.public_key),
                    "publicKey": BASE64.encode(member.public_key),
                    "status": format!("{:?}", member.status).to_lowercase(),
                    "endpoints": member.endpoint_ids,
                })
            })
            .collect::<Vec<_>>();
        json(&values).map_err(js_error)
    }

    #[wasm_bindgen(js_name = approvePeer)]
    pub async fn approve_peer(&self, fingerprint: String) -> Result<(), JsError> {
        self.decide_pending(&fingerprint, true)
            .await
            .map_err(js_error)
    }

    #[wasm_bindgen(js_name = rejectPeer)]
    pub async fn reject_peer(&self, fingerprint: String) -> Result<(), JsError> {
        self.decide_pending(&fingerprint, false)
            .await
            .map_err(js_error)
    }

    #[wasm_bindgen(js_name = removePeer)]
    pub async fn remove_peer(&self, fingerprint: String) -> Result<(), JsError> {
        let workspace = self.workspace().map_err(js_error)?;
        let registry = workspace.registry.read().await;
        let member = registry
            .member(&fingerprint)
            .filter(|member| member.status == MemberStatus::Active)
            .cloned()
            .context("active member is unavailable")
            .map_err(js_error)?;
        drop(registry);
        if member.public_key == self.identity.public_key() {
            return Err(JsError::new("cannot remove the current peer"));
        }
        let cutoff = workspace
            .signed
            .read()
            .await
            .values()
            .filter(|change| change.public_key == member.public_key)
            .map(|change| change.sequence)
            .max()
            .unwrap_or(0);
        let event = SignedMembershipEvent::create(
            &self.identity,
            WorkspaceId::new(workspace.id.clone()),
            now_hlc(&self.identity).map_err(js_error)?,
            MembershipEvent::PeerRemoved {
                peer_id: member.peer_id,
                public_key: member.public_key,
                accepted_actor_sequence: cutoff,
                accepted_heads: workspace.document.lock().await.clone().heads(),
                reason: Some("removed from xo-web".into()),
            },
        )
        .map_err(js_error)?;
        self.put_membership_event(&workspace, event)
            .await
            .map_err(js_error)?;
        workspace.gossip_sender.lock().await.take();
        start_browser_gossip(
            self.gossip.clone(),
            self.router.endpoint().clone(),
            self.identity.clone(),
            workspace,
        )
        .await
        .map_err(js_error)
    }

    #[wasm_bindgen(js_name = shareTicket)]
    pub async fn share_ticket(&mut self) -> Result<String, JsError> {
        let workspace = self.workspace().map_err(js_error)?;
        let invitation = WorkspaceInvitation {
            version: PROTOCOL_VERSION,
            workspace_id: workspace.id.clone(),
            bootstrap_peers: vec![self.router.endpoint().addr()],
            gossip_topic: *workspace.gossip_topic.read().await,
            base_gossip_topic: workspace.base_gossip_topic,
            membership_epoch: workspace.membership_epoch.load(Ordering::Acquire),
            genesis_key_fingerprint: workspace.genesis_fingerprint.clone(),
        };
        self.invitation = Some(invitation.clone());
        invitation.encode().map_err(js_error)
    }
}

impl IrohDocNode {
    fn workspace(&self) -> Result<Arc<BrowserWorkspace>> {
        self.workspace.clone().context("no workspace is loaded")
    }

    async fn ensure_join_workspace(&mut self, invitation: &WorkspaceInvitation) -> Result<()> {
        if self.workspace.is_some() {
            return Ok(());
        }
        let workspace = Arc::new(BrowserWorkspace {
            id: invitation.workspace_id.clone(),
            document: Mutex::new(AutomergeRecordStore::create(
                &invitation.workspace_id,
                &self.identity.public_key(),
            )?),
            registry: RwLock::new(MembershipRegistry::default()),
            pending: RwLock::new(BTreeMap::new()),
            signed: RwLock::new(BTreeMap::new()),
            peers: RwLock::new(
                invitation
                    .bootstrap_peers
                    .iter()
                    .cloned()
                    .map(|p| (p.id, p))
                    .collect(),
            ),
            genesis_fingerprint: invitation.genesis_key_fingerprint.clone(),
            gossip_topic: RwLock::new(invitation.gossip_topic),
            base_gossip_topic: invitation.base_gossip_topic,
            membership_epoch: AtomicU64::new(invitation.membership_epoch),
            gossip_sender: Mutex::new(None),
            identity: self.identity.clone(),
        });
        workspace.sign_local().await?;
        self.workspaces
            .write()
            .await
            .insert(workspace.id.clone(), workspace.clone());
        self.workspace = Some(workspace.clone());
        start_browser_gossip(
            self.gossip.clone(),
            self.router.endpoint().clone(),
            self.identity.clone(),
            workspace,
        )
        .await?;
        Ok(())
    }

    async fn request_join(&self, invitation: &WorkspaceInvitation) -> Result<JoinResponse> {
        let request = JoinRequest::create(
            &self.identity,
            invitation.workspace_id.clone(),
            self.router.endpoint().id().to_string(),
            vec![],
            now_ms()?,
        )?;
        let mut last = None;
        for peer in &invitation.bootstrap_peers {
            match self
                .router
                .endpoint()
                .connect(peer.clone(), JOIN_ALPN)
                .await
            {
                Ok(connection) => {
                    let (mut send, mut recv) = connection.open_bi().await?;
                    write_frame(&mut send, &encode(&request)?).await?;
                    send.finish()?;
                    return decode(&read_frame(&mut recv).await?).map_err(Into::into);
                }
                Err(error) => last = Some(error),
            }
        }
        Err(last
            .context("no invitation peer accepted the request")?
            .into())
    }

    async fn sync_invitation(&self, invitation: &WorkspaceInvitation) -> Result<()> {
        let workspace = self.workspace()?;
        for peer in &invitation.bootstrap_peers {
            if peer.id == self.router.endpoint().id() {
                continue;
            }
            sync_peer(
                self.router.endpoint(),
                &self.identity,
                &workspace,
                peer.clone(),
            )
            .await?;
        }
        Ok(())
    }

    async fn put_bytes(&self, key: String, value: Vec<u8>) -> Result<String> {
        if key.is_empty() {
            bail!("document key is required");
        }
        if value.len() > MAX_ENTRY_BYTES {
            bail!("document value exceeds 8 MiB");
        }
        let workspace = self.workspace()?;
        if !workspace
            .registry
            .read()
            .await
            .is_active_key(&self.identity.public_key())
        {
            bail!("local peer is not an active workspace member");
        }
        let hash = blake3::hash(&value).to_hex().to_string();
        workspace.document.lock().await.put(&key, value)?;
        workspace.sign_local().await?;
        announce_browser(&workspace, self.router.endpoint()).await?;
        if let Some(invitation) = &self.invitation {
            let _ = self.sync_invitation(invitation).await;
        }
        Ok(hash)
    }

    async fn decide_pending(&self, fingerprint: &str, approve: bool) -> Result<()> {
        let workspace = self.workspace()?;
        let request = workspace
            .pending
            .write()
            .await
            .remove(fingerprint)
            .context("pending member is unavailable")?;
        let payload = if approve {
            MembershipEvent::JoinApproved {
                peer_id: request.peer_id,
                public_key: request.public_key,
                endpoint_id: request.endpoint_id,
            }
        } else {
            MembershipEvent::JoinRejected {
                peer_id: request.peer_id,
                public_key: request.public_key,
            }
        };
        let event = SignedMembershipEvent::create(
            &self.identity,
            WorkspaceId::new(workspace.id.clone()),
            now_hlc(&self.identity)?,
            payload,
        )?;
        self.put_membership_event(&workspace, event).await
    }

    async fn put_membership_event(
        &self,
        workspace: &BrowserWorkspace,
        event: SignedMembershipEvent,
    ) -> Result<()> {
        workspace.document.lock().await.put(
            &format!("membership/event/{}", event.event_id),
            encode(&event)?,
        )?;
        workspace.sign_local().await?;
        workspace.refresh_registry().await?;
        if let Some(invitation) = &self.invitation {
            let _ = self.sync_invitation(invitation).await;
        }
        Ok(())
    }

    async fn register_browser_device(&self) -> Result<()> {
        let endpoint_id = self.router.endpoint().id().to_string();
        let device = DeviceRecord {
            schema: CURRENT_SCHEMA,
            endpoint_id: endpoint_id.clone(),
            author_id: self.identity.actor_id(),
            label: self.identity.peer_id().to_string(),
            capabilities: BTreeSet::from(["write".into(), "browser".into()]),
            last_seen_ms: Some(now_ms()?),
            retired_at: None,
        };
        let mut bytes = Vec::new();
        ciborium::into_writer(&device, &mut bytes)?;
        self.put_bytes(format!("device/{endpoint_id}"), bytes)
            .await?;
        Ok(())
    }
}

async fn start_browser_gossip(
    gossip: Gossip,
    endpoint: Endpoint,
    identity: Arc<MembershipIdentity>,
    workspace: Arc<BrowserWorkspace>,
) -> Result<()> {
    if workspace.gossip_sender.lock().await.is_some() {
        return Ok(());
    }
    let bootstrap = workspace
        .peers
        .read()
        .await
        .keys()
        .copied()
        .filter(|peer| *peer != endpoint.id())
        .collect::<Vec<_>>();
    let topic = gossip
        .subscribe(
            TopicId::from(*workspace.gossip_topic.read().await),
            bootstrap,
        )
        .await?;
    let (sender, mut receiver) = topic.split();
    *workspace.gossip_sender.lock().await = Some(sender);
    let receiving_workspace = workspace.clone();
    let receiving_endpoint = endpoint.clone();
    let receiving_identity = identity.clone();
    n0_future::task::spawn(async move {
        while let Ok(Some(event)) = futures_lite::StreamExt::try_next(&mut receiver).await {
            let peer = match event {
                GossipEvent::NeighborUp(endpoint_id) => Some(endpoint_id),
                GossipEvent::Received(message) => {
                    let Ok(announcement) = decode::<GossipAnnouncement>(&message.content) else {
                        continue;
                    };
                    if announcement.verify().is_err()
                        || announcement.workspace_id != receiving_workspace.id
                        || !receiving_workspace
                            .registry
                            .read()
                            .await
                            .is_active_key(&announcement.public_key)
                    {
                        continue;
                    }
                    announcement.endpoint_id.parse().ok()
                }
                GossipEvent::NeighborDown(_) | GossipEvent::Lagged => None,
            };
            let Some(peer) = peer else {
                continue;
            };
            let active = receiving_workspace
                .registry
                .read()
                .await
                .members()
                .any(|member| {
                    member.status == MemberStatus::Active
                        && member.endpoint_ids.contains(&peer.to_string())
                });
            if !active || peer == receiving_endpoint.id() {
                continue;
            }
            let address = EndpointAddr::new(peer);
            receiving_workspace
                .peers
                .write()
                .await
                .insert(peer, address.clone());
            let _ = sync_peer(
                &receiving_endpoint,
                &receiving_identity,
                &receiving_workspace,
                address,
            )
            .await;
        }
    });
    announce_browser(&workspace, &endpoint).await
}

async fn announce_browser(workspace: &BrowserWorkspace, endpoint: &Endpoint) -> Result<()> {
    let heads = workspace.document.lock().await.clone().heads();
    let announcement = GossipAnnouncement::create(
        &workspace.identity,
        workspace.id.clone(),
        endpoint.id().to_string(),
        heads,
        now_ms()?,
    )?;
    if let Some(sender) = workspace.gossip_sender.lock().await.as_ref() {
        sender.broadcast(encode(&announcement)?.into()).await?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct BrowserJoin {
    workspaces: WorkspaceMap,
    identity: Arc<MembershipIdentity>,
}
impl ProtocolHandler for BrowserJoin {
    async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), AcceptError> {
        async {
            let (mut send, mut recv) = connection.accept_bi().await?;
            let request: JoinRequest = decode(&read_frame(&mut recv).await?)?;
            request.verify()?;
            if request.endpoint_id != connection.remote_id().to_string() {
                bail!("join endpoint mismatch");
            }
            let workspace = self
                .workspaces
                .read()
                .await
                .get(&request.workspace_id)
                .cloned()
                .context("unknown workspace")?;
            let fingerprint = public_key_fingerprint(&request.public_key);
            let member_status = workspace
                .registry
                .read()
                .await
                .member(&fingerprint)
                .map(|member| member.status);
            let response = match member_status {
                Some(MemberStatus::Active) => JoinResponse::Approved {
                    membership_event: vec![],
                },
                Some(_) => JoinResponse::Rejected,
                None => {
                    let event = SignedMembershipEvent::create(
                        &self.identity,
                        WorkspaceId::new(workspace.id.clone()),
                        now_hlc(&self.identity)?,
                        MembershipEvent::JoinApproved {
                            peer_id: request.peer_id,
                            public_key: request.public_key,
                            endpoint_id: request.endpoint_id,
                        },
                    )?;
                    workspace.document.lock().await.put(
                        &format!("membership/event/{}", event.event_id),
                        encode(&event)?,
                    )?;
                    workspace.sign_local().await?;
                    workspace.refresh_registry().await?;
                    JoinResponse::Approved {
                        membership_event: encode(&event)?,
                    }
                }
            };
            write_frame(&mut send, &encode(&response)?).await?;
            send.finish()?;
            Ok::<_, anyhow::Error>(())
        }
        .await
        .map_err(|error| AcceptError::from_boxed(error.into()))
    }
}

#[derive(Clone, Debug)]
struct BrowserSync {
    workspaces: WorkspaceMap,
    identity: Arc<MembershipIdentity>,
    endpoint_id: iroh::EndpointId,
}
impl ProtocolHandler for BrowserSync {
    async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), AcceptError> {
        async {
            let (mut send, mut recv) = connection.accept_bi().await?;
            let id: String = decode(&read_frame(&mut recv).await?)?;
            let challenge: [u8; 32] = rand::random();
            write_frame(&mut send, &encode(&challenge)?).await?;
            let remote: AuthHello = decode(&read_frame(&mut recv).await?)?;
            remote.verify()?;
            if remote.remote_nonce != challenge
                || remote.endpoint_id != connection.remote_id().to_string()
                || remote.workspace_id != id
            {
                bail!("authentication transcript mismatch");
            }
            let workspace = self
                .workspaces
                .read()
                .await
                .get(&id)
                .cloned()
                .context("unknown workspace")?;
            let registry = workspace.registry.read().await;
            let bootstrap = registry.members().next().is_none()
                && public_key_fingerprint(&remote.public_key) == workspace.genesis_fingerprint;
            if !registry.is_active_key(&remote.public_key) && !bootstrap {
                bail!("remote peer is not active");
            }
            drop(registry);
            let local = AuthHello::create(
                &self.identity,
                id,
                self.endpoint_id.to_string(),
                remote.nonce,
            )?;
            write_frame(&mut send, &encode(&local)?).await?;
            let payload = decode(&read_frame(&mut recv).await?)?;
            workspace.apply(payload).await?;
            write_frame(&mut send, &encode(&workspace.payload().await?)?).await?;
            send.finish()?;
            Ok::<_, anyhow::Error>(())
        }
        .await
        .map_err(|error| AcceptError::from_boxed(error.into()))
    }
}

async fn sync_peer(
    endpoint: &Endpoint,
    identity: &MembershipIdentity,
    workspace: &BrowserWorkspace,
    peer: EndpointAddr,
) -> Result<()> {
    let connection = endpoint.connect(peer.clone(), AUTOMERGE_ALPN).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    write_frame(&mut send, &encode(&workspace.id)?).await?;
    let challenge: [u8; 32] = decode(&read_frame(&mut recv).await?)?;
    let hello = AuthHello::create(
        identity,
        workspace.id.clone(),
        endpoint.id().to_string(),
        challenge,
    )?;
    write_frame(&mut send, &encode(&hello)?).await?;
    let response: AuthHello = decode(&read_frame(&mut recv).await?)?;
    response.verify()?;
    if response.remote_nonce != hello.nonce || response.endpoint_id != peer.id.to_string() {
        bail!("sync authentication mismatch");
    }
    write_frame(&mut send, &encode(&workspace.payload().await?)?).await?;
    send.finish()?;
    let payload = decode(&read_frame(&mut recv).await?)?;
    workspace.apply(payload).await?;
    Ok(())
}

async fn write_frame(send: &mut iroh::endpoint::SendStream, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_PROTOCOL_FRAME {
        bail!("protocol frame is too large");
    }
    send.write_all(&u32::try_from(bytes.len())?.to_be_bytes())
        .await?;
    send.write_all(bytes).await?;
    Ok(())
}
async fn read_frame(recv: &mut iroh::endpoint::RecvStream) -> Result<Vec<u8>> {
    let mut encoded_size = [0_u8; 4];
    recv.read_exact(&mut encoded_size).await?;
    let size = usize::try_from(u32::from_be_bytes(encoded_size))?;
    if size > MAX_PROTOCOL_FRAME {
        bail!("protocol frame is too large");
    }
    let mut bytes = vec![0; size];
    recv.read_exact(&mut bytes).await?;
    Ok(bytes)
}

fn valid_workspace_id() -> String {
    ed25519_dalek::SigningKey::from_bytes(&rand::random())
        .verifying_key()
        .to_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}
fn now_ms() -> Result<u64> {
    js_sys::Date::now()
        .round()
        .to_string()
        .parse()
        .context("browser time is invalid")
}
fn now_hlc(identity: &MembershipIdentity) -> Result<Hlc> {
    Ok(Hlc {
        physical_ms: now_ms()?,
        logical: 0,
        actor_id: identity.actor_id(),
    })
}

fn record_author(key: &str, value: &[u8]) -> Option<String> {
    if key.contains("/revision/") {
        return ciborium::from_reader::<xo_core::NoteRevision, _>(value)
            .ok()
            .map(|v| v.author_id.to_string());
    }
    if key.contains("/head/") {
        return ciborium::from_reader::<xo_core::Head, _>(value)
            .ok()
            .map(|v| v.author_id.to_string());
    }
    if key.starts_with("config/") {
        return ciborium::from_reader::<xo_core::ConfigRevision, _>(value)
            .ok()
            .map(|v| v.author_id.to_string());
    }
    if key.starts_with("tombstone/") {
        return ciborium::from_reader::<xo_core::Tombstone, _>(value)
            .ok()
            .map(|v| v.author_id.to_string());
    }
    if key.starts_with("device/") {
        return ciborium::from_reader::<DeviceRecord, _>(value)
            .ok()
            .map(|v| v.author_id.to_string());
    }
    None
}

#[derive(Debug)]
struct NormalizedPkarrResolverBuilder(iroh::address_lookup::PkarrResolverBuilder);
impl AddressLookupBuilder for NormalizedPkarrResolverBuilder {
    fn into_address_lookup(
        self,
        endpoint: &Endpoint,
    ) -> Result<impl AddressLookup, AddressLookupBuilderError> {
        Ok(NormalizedPkarrResolver(
            self.0.into_address_lookup(endpoint)?,
        ))
    }
}
#[derive(Debug)]
struct NormalizedPkarrResolver<T>(T);
impl<T: AddressLookup> AddressLookup for NormalizedPkarrResolver<T> {
    fn resolve(
        &self,
        endpoint_id: iroh::EndpointId,
    ) -> Option<n0_future::boxed::BoxStream<Result<AddressLookupItem, AddressLookupError>>> {
        self.0.resolve(endpoint_id).map(|stream| {
            Box::pin(futures_lite::StreamExt::map(stream, |item| {
                item.map(normalize_lookup_item)
            })) as _
        })
    }
}
fn normalize_lookup_item(item: AddressLookupItem) -> AddressLookupItem {
    let provenance = item.provenance();
    let last_updated = item.last_updated();
    let mut address = item.to_endpoint_addr();
    if normalize_browser_nodes(std::slice::from_mut(&mut address)).is_err() {
        return item;
    }
    AddressLookupItem::new(EndpointInfo::from(address), provenance, last_updated)
}
fn normalize_browser_nodes(nodes: &mut [EndpointAddr]) -> Result<()> {
    for node in nodes {
        node.addrs = std::mem::take(&mut node.addrs)
            .into_iter()
            .filter_map(|address| match address {
                TransportAddr::Relay(relay) => {
                    Some(normalize_relay_url(relay).map(TransportAddr::Relay))
                }
                _ => None,
            })
            .collect::<Result<_>>()?;
    }
    Ok(())
}
fn browser_relay_map() -> Result<RelayMap> {
    let urls = RelayMode::Default
        .relay_map()
        .urls::<Vec<_>>()
        .into_iter()
        .map(normalize_relay_url)
        .collect::<Result<Vec<_>>>()?;
    Ok(RelayMode::custom(urls).relay_map())
}
fn relay_map_for_nodes(nodes: &[EndpointAddr]) -> RelayMap {
    RelayMode::custom(
        nodes
            .iter()
            .flat_map(|node| node.relay_urls().cloned())
            .collect::<Vec<_>>(),
    )
    .relay_map()
}
fn normalize_relay_url(relay: RelayUrl) -> Result<RelayUrl> {
    let mut url: url::Url = relay.into();
    if let Some(host) = url.host_str().and_then(|host| host.strip_suffix('.')) {
        let host = host.to_owned();
        url.set_host(Some(&host))?;
    }
    Ok(url.into())
}
fn fixed_secret(value: &Uint8Array, label: &str) -> Result<[u8; 32], JsError> {
    value
        .to_vec()
        .try_into()
        .map_err(|_| JsError::new(&format!("{label} secret must contain exactly 32 bytes")))
}
fn json(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).context("encode browser Iroh state")
}
fn js_error(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}
