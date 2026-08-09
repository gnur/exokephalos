//! Native Automerge workspace transport over authenticated Iroh QUIC streams.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use futures_lite::StreamExt as _;
use iroh::endpoint::presets;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, SecretKey};
use iroh_gossip::ALPN as GOSSIP_ALPN;
use iroh_gossip::TopicId;
use iroh_gossip::api::{Event as GossipEvent, GossipSender};
use iroh_gossip::net::Gossip;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::{Mutex, RwLock, broadcast};

use crate::automerge_store::PersistentAutomergeStore;
use crate::membership::{
    MemberStatus, MembershipEvent, MembershipIdentity, MembershipRegistry, PeerId,
    SignedMembershipEvent, load_or_create_identity,
};
use crate::peer_protocol::{
    AUTOMERGE_ALPN, AuthHello, GossipAnnouncement, JOIN_ALPN, JoinRequest, JoinResponse,
    MAX_PROTOCOL_FRAME, PROTOCOL_VERSION, WorkspaceInvitation, decode, encode,
};
use crate::{ActorId, Hlc, WorkspaceId};

const ENDPOINT_KEY_FILE: &str = "endpoint.key";
const WORKSPACES_DIR: &str = "automerge-workspaces";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutomergeWorkspaceEvent {
    ContentChanged,
    MembershipChanged,
    StatusChanged,
}

#[derive(Debug)]
struct WorkspaceState {
    id: String,
    store: Mutex<PersistentAutomergeStore>,
    registry: RwLock<MembershipRegistry>,
    pending: RwLock<BTreeMap<String, JoinRequest>>,
    peers: RwLock<BTreeMap<EndpointId, EndpointAddr>>,
    events: broadcast::Sender<AutomergeWorkspaceEvent>,
    genesis_fingerprint: String,
    gossip_topic: [u8; 32],
    gossip_sender: Mutex<Option<GossipSender>>,
}

impl WorkspaceState {
    async fn refresh_registry(&self) -> Result<()> {
        let encoded = self.store.lock().await.store().scan("membership/event/")?;
        let events = encoded
            .values()
            .map(|bytes| decode::<SignedMembershipEvent>(bytes).map_err(anyhow::Error::from))
            .collect::<Result<Vec<_>>>()?;
        let mut remaining = events;
        let mut registry = MembershipRegistry::default();
        while !remaining.is_empty() {
            let previous = remaining.len();
            remaining.retain(|event| registry.apply(event).is_err());
            if remaining.len() == previous {
                bail!("membership event graph contains unauthorized or invalid events");
            }
        }
        *self.registry.write().await = registry;
        Ok(())
    }

    async fn put_membership_event(&self, event: &SignedMembershipEvent) -> Result<()> {
        let key = format!("membership/event/{}", event.event_id);
        self.store.lock().await.put(&key, encode(event)?)?;
        self.refresh_registry().await?;
        let _ = self.events.send(AutomergeWorkspaceEvent::MembershipChanged);
        Ok(())
    }
}

type WorkspaceMap = Arc<RwLock<BTreeMap<String, Arc<WorkspaceState>>>>;

/// Persistent Iroh endpoint hosting custom admission, Automerge sync, and Gossip protocols.
#[derive(Debug)]
pub struct AutomergeNode {
    router: Router,
    identity: Arc<MembershipIdentity>,
    workspaces: WorkspaceMap,
    gossip: Gossip,
    state_dir: PathBuf,
}

impl AutomergeNode {
    pub async fn persistent(state_dir: impl AsRef<Path>, peer_id: PeerId) -> Result<Self> {
        Self::persistent_inner(state_dir, peer_id, None).await
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn persistent_with_relay_map(
        state_dir: impl AsRef<Path>,
        peer_id: PeerId,
        relay_map: RelayMap,
    ) -> Result<Self> {
        Self::persistent_inner(state_dir, peer_id, Some(relay_map)).await
    }

    async fn persistent_inner(
        state_dir: impl AsRef<Path>,
        peer_id: PeerId,
        #[allow(unused_variables)] relay_map: Option<RelayMap>,
    ) -> Result<Self> {
        let state_dir = state_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&state_dir)?;
        let identity = Arc::new(load_or_create_identity(&state_dir, &peer_id)?);
        let endpoint_key = load_or_create_endpoint_key(&state_dir.join(ENDPOINT_KEY_FILE))?;
        #[allow(unused_mut)]
        let mut builder = Endpoint::builder(presets::N0).secret_key(endpoint_key);
        #[cfg(any(test, feature = "test-utils"))]
        if let Some(relay_map) = relay_map {
            builder = builder
                .relay_mode(iroh::RelayMode::Custom(relay_map))
                .ca_roots_config(iroh::tls::CaRootsConfig::insecure_skip_verify());
        }
        let endpoint = builder.bind().await.context("bind Iroh endpoint")?;
        let workspaces = Arc::new(RwLock::new(BTreeMap::new()));
        load_workspaces(&state_dir, &identity, &workspaces).await?;
        let join = JoinProtocol {
            workspaces: workspaces.clone(),
        };
        let sync = SyncProtocol {
            workspaces: workspaces.clone(),
            identity: identity.clone(),
            local_endpoint: endpoint.id(),
        };
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let router = Router::builder(endpoint)
            .accept(JOIN_ALPN, join)
            .accept(AUTOMERGE_ALPN, sync)
            .accept(GOSSIP_ALPN, gossip.clone())
            .spawn();
        let node = Self {
            router,
            identity,
            workspaces,
            gossip,
            state_dir,
        };
        let existing = node
            .workspaces
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for state in existing {
            node.workspace(state).start_gossip().await?;
        }
        Ok(node)
    }

    #[must_use]
    pub fn endpoint_id(&self) -> EndpointId {
        self.router.endpoint().id()
    }

    #[must_use]
    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.router.endpoint().addr()
    }

    #[must_use]
    pub fn peer_id(&self) -> &PeerId {
        self.identity.peer_id()
    }

    #[must_use]
    pub fn membership_fingerprint(&self) -> String {
        self.identity.fingerprint()
    }

    pub async fn workspace_ids(&self) -> Vec<String> {
        self.workspaces.read().await.keys().cloned().collect()
    }

    pub async fn create_workspace(&self) -> Result<AutomergeWorkspace> {
        let id = blake3::hash(&rand::random::<[u8; 32]>())
            .to_hex()
            .to_string();
        let topic: [u8; 32] = rand::random();
        let directory = self.state_dir.join(WORKSPACES_DIR).join(&id);
        let mut store =
            PersistentAutomergeStore::open_or_create(&directory, &id, &self.identity.public_key())?;
        let issued_at = timestamp(&self.identity)?;
        let genesis = SignedMembershipEvent::create(
            &self.identity,
            WorkspaceId::new(id.clone()),
            issued_at,
            MembershipEvent::Genesis {
                peer_id: self.identity.peer_id().clone(),
                public_key: self.identity.public_key(),
                endpoint_id: self.endpoint_id().to_string(),
            },
        )?;
        store.put(
            &format!("membership/event/{}", genesis.event_id),
            encode(&genesis)?,
        )?;
        let (events, _) = broadcast::channel(256);
        let state = Arc::new(WorkspaceState {
            id: id.clone(),
            store: Mutex::new(store),
            registry: RwLock::new(MembershipRegistry::default()),
            pending: RwLock::new(BTreeMap::new()),
            peers: RwLock::new(BTreeMap::new()),
            events,
            genesis_fingerprint: self.identity.fingerprint(),
            gossip_topic: topic,
            gossip_sender: Mutex::new(None),
        });
        state.refresh_registry().await?;
        write_workspace_metadata(&directory, &state)?;
        self.workspaces.write().await.insert(id, state.clone());
        let workspace = self.workspace(state);
        workspace.start_gossip().await?;
        Ok(workspace)
    }

    pub async fn open_workspace(&self, id: &str) -> Option<AutomergeWorkspace> {
        self.workspaces
            .read()
            .await
            .get(id)
            .cloned()
            .map(|state| self.workspace(state))
    }

    pub async fn request_join(&self, invitation: &str) -> Result<JoinResponse> {
        let invitation = WorkspaceInvitation::decode(invitation)?;
        let request = JoinRequest::create(
            &self.identity,
            invitation.workspace_id.clone(),
            self.endpoint_id().to_string(),
            vec![],
            now_ms()?,
        )?;
        let mut last_error = None;
        for peer in &invitation.bootstrap_peers {
            match self.send_join_request(peer.clone(), &request).await {
                Ok(response) => return Ok(response),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.context("no invitation bootstrap peer accepted the join request")?)
    }

    async fn send_join_request(
        &self,
        peer: EndpointAddr,
        request: &JoinRequest,
    ) -> Result<JoinResponse> {
        let connection = self
            .router
            .endpoint()
            .connect(peer, JOIN_ALPN)
            .await
            .context("connect to admission peer")?;
        let (mut send, mut recv) = connection.open_bi().await?;
        write_frame(&mut send, &encode(request)?).await?;
        send.finish()?;
        let response = decode(&read_frame(&mut recv).await?).map_err(Into::into);
        connection.close(0_u32.into(), b"join complete");
        response
    }

    pub async fn import_approved_workspace(&self, invitation: &str) -> Result<AutomergeWorkspace> {
        let invitation = WorkspaceInvitation::decode(invitation)?;
        let directory = self
            .state_dir
            .join(WORKSPACES_DIR)
            .join(&invitation.workspace_id);
        let store = PersistentAutomergeStore::open_or_create(
            &directory,
            &invitation.workspace_id,
            &self.identity.public_key(),
        )?;
        let (events, _) = broadcast::channel(256);
        let state = Arc::new(WorkspaceState {
            id: invitation.workspace_id.clone(),
            store: Mutex::new(store),
            registry: RwLock::new(MembershipRegistry::default()),
            pending: RwLock::new(BTreeMap::new()),
            peers: RwLock::new(
                invitation
                    .bootstrap_peers
                    .iter()
                    .cloned()
                    .map(|peer| (peer.id, peer))
                    .collect(),
            ),
            events,
            genesis_fingerprint: invitation.genesis_key_fingerprint,
            gossip_topic: invitation.gossip_topic,
            gossip_sender: Mutex::new(None),
        });
        write_workspace_metadata(&directory, &state)?;
        self.workspaces
            .write()
            .await
            .insert(state.id.clone(), state.clone());
        let workspace = self.workspace(state);
        workspace.start_gossip().await?;
        Ok(workspace)
    }

    pub async fn shutdown(&self) -> Result<()> {
        for workspace in self.workspaces.read().await.values() {
            workspace.store.lock().await.flush()?;
        }
        self.router.shutdown().await.context("shutdown Iroh router")
    }

    fn workspace(&self, state: Arc<WorkspaceState>) -> AutomergeWorkspace {
        AutomergeWorkspace {
            state,
            endpoint: self.router.endpoint().clone(),
            identity: self.identity.clone(),
            gossip: self.gossip.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AutomergeWorkspace {
    state: Arc<WorkspaceState>,
    endpoint: Endpoint,
    identity: Arc<MembershipIdentity>,
    gossip: Gossip,
}

impl AutomergeWorkspace {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.state.id
    }

    pub fn invitation(&self) -> Result<String> {
        WorkspaceInvitation {
            version: PROTOCOL_VERSION,
            workspace_id: self.state.id.clone(),
            bootstrap_peers: vec![self.endpoint.addr()],
            gossip_topic: self.state.gossip_topic,
            genesis_key_fingerprint: self.state.genesis_fingerprint.clone(),
        }
        .encode()
        .map_err(Into::into)
    }

    async fn start_gossip(&self) -> Result<()> {
        if self.state.gossip_sender.lock().await.is_some() {
            return Ok(());
        }
        let bootstrap = self
            .state
            .peers
            .read()
            .await
            .keys()
            .copied()
            .filter(|peer| *peer != self.endpoint.id())
            .collect::<Vec<_>>();
        let topic = self
            .gossip
            .subscribe(TopicId::from(self.state.gossip_topic), bootstrap)
            .await?;
        let (sender, mut receiver) = topic.split();
        *self.state.gossip_sender.lock().await = Some(sender);
        let workspace = self.clone();
        tokio::spawn(async move {
            while let Ok(Some(event)) = receiver.try_next().await {
                match event {
                    GossipEvent::NeighborUp(endpoint_id) => {
                        if workspace.active_endpoint(endpoint_id).await {
                            workspace
                                .state
                                .peers
                                .write()
                                .await
                                .insert(endpoint_id, EndpointAddr::new(endpoint_id));
                            let syncing = workspace.clone();
                            tokio::spawn(async move {
                                let _ = syncing.sync_peer(EndpointAddr::new(endpoint_id)).await;
                            });
                        }
                        let _ = workspace
                            .state
                            .events
                            .send(AutomergeWorkspaceEvent::StatusChanged);
                    }
                    GossipEvent::NeighborDown(_) | GossipEvent::Lagged => {
                        let _ = workspace
                            .state
                            .events
                            .send(AutomergeWorkspaceEvent::StatusChanged);
                    }
                    GossipEvent::Received(message) => {
                        let Ok(announcement) = decode::<GossipAnnouncement>(&message.content)
                        else {
                            continue;
                        };
                        if announcement.verify().is_err()
                            || announcement.workspace_id != workspace.state.id
                            || !workspace
                                .state
                                .registry
                                .read()
                                .await
                                .is_active_key(&announcement.public_key)
                        {
                            continue;
                        }
                        let Ok(endpoint_id) = announcement.endpoint_id.parse::<EndpointId>() else {
                            continue;
                        };
                        workspace
                            .state
                            .peers
                            .write()
                            .await
                            .insert(endpoint_id, EndpointAddr::new(endpoint_id));
                        let local_heads =
                            workspace.state.store.lock().await.store().clone().heads();
                        if local_heads != announcement.heads {
                            let syncing = workspace.clone();
                            tokio::spawn(async move {
                                let _ = syncing.sync_peer(EndpointAddr::new(endpoint_id)).await;
                            });
                        }
                    }
                }
            }
        });
        self.announce().await
    }

    async fn active_endpoint(&self, endpoint_id: EndpointId) -> bool {
        let endpoint = endpoint_id.to_string();
        self.state.registry.read().await.members().any(|member| {
            member.status == MemberStatus::Active && member.endpoint_ids.contains(&endpoint)
        })
    }

    async fn announce(&self) -> Result<()> {
        let heads = self.state.store.lock().await.store().clone().heads();
        let announcement = GossipAnnouncement::create(
            &self.identity,
            self.state.id.clone(),
            self.endpoint.id().to_string(),
            heads,
            now_ms()?,
        )?;
        if let Some(sender) = self.state.gossip_sender.lock().await.as_ref() {
            sender.broadcast(encode(&announcement)?.into()).await?;
        }
        Ok(())
    }

    pub async fn pending_requests(&self) -> Vec<JoinRequest> {
        self.state.pending.read().await.values().cloned().collect()
    }

    pub async fn approve(&self, public_key: &[u8; 32]) -> Result<SignedMembershipEvent> {
        let fingerprint = crate::membership::public_key_fingerprint(public_key);
        let request = self
            .state
            .pending
            .write()
            .await
            .remove(&fingerprint)
            .context("pending membership request is unavailable")?;
        let event = SignedMembershipEvent::create(
            &self.identity,
            WorkspaceId::new(self.state.id.clone()),
            timestamp(&self.identity)?,
            MembershipEvent::JoinApproved {
                peer_id: request.peer_id,
                public_key: request.public_key,
                endpoint_id: request.endpoint_id,
            },
        )?;
        self.state.put_membership_event(&event).await?;
        self.announce().await?;
        Ok(event)
    }

    pub async fn remove(
        &self,
        public_key: &[u8; 32],
        reason: Option<String>,
    ) -> Result<SignedMembershipEvent> {
        let registry = self.state.registry.read().await;
        let member = registry
            .member(&crate::membership::public_key_fingerprint(public_key))
            .filter(|member| member.status == MemberStatus::Active)
            .context("active member is unavailable")?
            .clone();
        drop(registry);
        let heads = self.state.store.lock().await.store().clone().heads();
        let event = SignedMembershipEvent::create(
            &self.identity,
            WorkspaceId::new(self.state.id.clone()),
            timestamp(&self.identity)?,
            MembershipEvent::PeerRemoved {
                peer_id: member.peer_id,
                public_key: member.public_key,
                accepted_actor_sequence: 0,
                accepted_heads: heads,
                reason,
            },
        )?;
        self.state.put_membership_event(&event).await?;
        self.announce().await?;
        Ok(event)
    }

    pub async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.state.store.lock().await.put(key, value)?;
        self.announce().await?;
        let _ = self
            .state
            .events
            .send(AutomergeWorkspaceEvent::ContentChanged);
        Ok(())
    }

    pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.state.store.lock().await.store().get(key)?)
    }

    pub async fn scan(&self, prefix: &str) -> Result<BTreeMap<String, Vec<u8>>> {
        Ok(self.state.store.lock().await.store().scan(prefix)?)
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<AutomergeWorkspaceEvent> {
        self.state.events.subscribe()
    }

    pub async fn sync(&self) -> Result<()> {
        let peers = self
            .state
            .peers
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut successes = 0;
        let mut last_error = None;
        for peer in peers {
            if peer.id == self.endpoint.id() {
                continue;
            }
            match self.sync_peer(peer).await {
                Ok(()) => successes += 1,
                Err(error) => last_error = Some(error),
            }
        }
        if successes == 0
            && let Some(error) = last_error
        {
            return Err(error);
        }
        Ok(())
    }

    async fn sync_peer(&self, peer: EndpointAddr) -> Result<()> {
        let connection = self.endpoint.connect(peer.clone(), AUTOMERGE_ALPN).await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        write_frame(&mut send, &encode(&self.state.id)?).await?;
        let challenge: [u8; 32] = decode(&read_frame(&mut recv).await?)?;
        let hello = AuthHello::create(
            &self.identity,
            self.state.id.clone(),
            self.endpoint.id().to_string(),
            challenge,
        )?;
        write_frame(&mut send, &encode(&hello)?).await?;
        let response: AuthHello = decode(&read_frame(&mut recv).await?)?;
        response.verify()?;
        if response.remote_nonce != hello.nonce || response.endpoint_id != peer.id.to_string() {
            bail!("sync peer authentication transcript does not match the connection");
        }
        let bootstrap_trusted = crate::membership::public_key_fingerprint(&response.public_key)
            == self.state.genesis_fingerprint;
        let active = self
            .state
            .registry
            .read()
            .await
            .is_active_key(&response.public_key);
        if !active && !bootstrap_trusted {
            bail!("sync peer is not an active workspace member");
        }
        let snapshot = self.state.store.lock().await.snapshot();
        write_frame(&mut send, &snapshot).await?;
        send.finish()?;
        let remote_snapshot = read_frame(&mut recv).await?;
        self.state
            .store
            .lock()
            .await
            .merge_snapshot(&remote_snapshot, &self.identity.public_key())?;
        self.state.refresh_registry().await?;
        let _ = self
            .state
            .events
            .send(AutomergeWorkspaceEvent::ContentChanged);
        connection.close(0_u32.into(), b"sync complete");
        Ok(())
    }

    pub async fn members(&self) -> Vec<crate::membership::Member> {
        self.state
            .registry
            .read()
            .await
            .members()
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug)]
struct JoinProtocol {
    workspaces: WorkspaceMap,
}

impl ProtocolHandler for JoinProtocol {
    async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), AcceptError> {
        self.handle(connection)
            .await
            .map_err(|error| AcceptError::from_boxed(error.into()))
    }
}

impl JoinProtocol {
    async fn handle(&self, connection: iroh::endpoint::Connection) -> Result<()> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let request: JoinRequest = decode(&read_frame(&mut recv).await?)?;
        request.verify()?;
        if request.endpoint_id != connection.remote_id().to_string() {
            bail!("join endpoint binding does not match the Iroh connection");
        }
        let workspace = self
            .workspaces
            .read()
            .await
            .get(&request.workspace_id)
            .cloned()
            .context("unknown workspace")?;
        let fingerprint = crate::membership::public_key_fingerprint(&request.public_key);
        let response = match workspace.registry.read().await.member(&fingerprint) {
            Some(member) if member.status == MemberStatus::Active => JoinResponse::Approved {
                membership_event: Vec::new(),
            },
            Some(member) if member.status != MemberStatus::Active => JoinResponse::Rejected,
            _ => {
                workspace.pending.write().await.insert(fingerprint, request);
                let _ = workspace
                    .events
                    .send(AutomergeWorkspaceEvent::MembershipChanged);
                JoinResponse::Pending
            }
        };
        write_frame(&mut send, &encode(&response)?).await?;
        send.finish()?;
        connection.closed().await;
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct SyncProtocol {
    workspaces: WorkspaceMap,
    identity: Arc<MembershipIdentity>,
    local_endpoint: EndpointId,
}

impl ProtocolHandler for SyncProtocol {
    async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), AcceptError> {
        self.handle(connection)
            .await
            .map_err(|error| AcceptError::from_boxed(error.into()))
    }
}

impl SyncProtocol {
    async fn handle(&self, connection: iroh::endpoint::Connection) -> Result<()> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let requested_workspace: String = decode(&read_frame(&mut recv).await?)?;
        let challenge: [u8; 32] = rand::random();
        write_frame(&mut send, &encode(&challenge)?).await?;
        let remote: AuthHello = decode(&read_frame(&mut recv).await?)?;
        remote.verify()?;
        if remote.remote_nonce != challenge
            || remote.endpoint_id != connection.remote_id().to_string()
            || remote.workspace_id != requested_workspace
        {
            bail!("remote authentication transcript does not match the Iroh connection");
        }
        let workspace = self
            .workspaces
            .read()
            .await
            .get(&remote.workspace_id)
            .cloned()
            .context("unknown workspace")?;
        if !workspace
            .registry
            .read()
            .await
            .is_active_key(&remote.public_key)
        {
            bail!("remote peer is not an active workspace member");
        }
        let local = AuthHello::create(
            &self.identity,
            workspace.id.clone(),
            self.local_endpoint.to_string(),
            remote.nonce,
        )?;
        write_frame(&mut send, &encode(&local)?).await?;
        let remote_snapshot = read_frame(&mut recv).await?;
        workspace
            .store
            .lock()
            .await
            .merge_snapshot(&remote_snapshot, &self.identity.public_key())?;
        workspace.refresh_registry().await?;
        let snapshot = workspace.store.lock().await.snapshot();
        write_frame(&mut send, &snapshot).await?;
        send.finish()?;
        let _ = workspace
            .events
            .send(AutomergeWorkspaceEvent::ContentChanged);
        connection.closed().await;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct WorkspaceMetadata {
    workspace_id: String,
    genesis_fingerprint: String,
    gossip_topic: [u8; 32],
    peers: Vec<EndpointAddr>,
}

async fn load_workspaces(
    state_dir: &Path,
    identity: &MembershipIdentity,
    workspaces: &WorkspaceMap,
) -> Result<()> {
    let root = state_dir.join(WORKSPACES_DIR);
    std::fs::create_dir_all(&root)?;
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let metadata: WorkspaceMetadata =
            serde_json::from_slice(&std::fs::read(entry.path().join("metadata.json"))?)?;
        let store = PersistentAutomergeStore::open_or_create(
            &entry.path(),
            &metadata.workspace_id,
            &identity.public_key(),
        )?;
        let (events, _) = broadcast::channel(256);
        let state = Arc::new(WorkspaceState {
            id: metadata.workspace_id.clone(),
            store: Mutex::new(store),
            registry: RwLock::new(MembershipRegistry::default()),
            pending: RwLock::new(BTreeMap::new()),
            peers: RwLock::new(
                metadata
                    .peers
                    .into_iter()
                    .map(|peer| (peer.id, peer))
                    .collect(),
            ),
            events,
            genesis_fingerprint: metadata.genesis_fingerprint,
            gossip_topic: metadata.gossip_topic,
            gossip_sender: Mutex::new(None),
        });
        state.refresh_registry().await?;
        workspaces
            .write()
            .await
            .insert(metadata.workspace_id, state);
    }
    Ok(())
}

fn write_workspace_metadata(directory: &Path, state: &WorkspaceState) -> Result<()> {
    let metadata = WorkspaceMetadata {
        workspace_id: state.id.clone(),
        genesis_fingerprint: state.genesis_fingerprint.clone(),
        gossip_topic: state.gossip_topic,
        peers: Vec::new(),
    };
    let temporary = directory.join("metadata.json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(&metadata)?)?;
    std::fs::rename(temporary, directory.join("metadata.json"))?;
    Ok(())
}

async fn write_frame(send: &mut iroh::endpoint::SendStream, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_PROTOCOL_FRAME {
        bail!("protocol frame exceeds {MAX_PROTOCOL_FRAME} bytes");
    }
    send.write_u32(u32::try_from(bytes.len())?).await?;
    send.write_all(bytes).await?;
    Ok(())
}

async fn read_frame(recv: &mut iroh::endpoint::RecvStream) -> Result<Vec<u8>> {
    let size = usize::try_from(recv.read_u32().await?)?;
    if size > MAX_PROTOCOL_FRAME {
        bail!("protocol frame exceeds {MAX_PROTOCOL_FRAME} bytes");
    }
    let mut bytes = vec![0; size];
    recv.read_exact(&mut bytes).await?;
    Ok(bytes)
}

fn load_or_create_endpoint_key(path: &Path) -> Result<SecretKey> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("endpoint key must contain exactly 32 bytes"))?;
            Ok(SecretKey::from_bytes(&bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let key = SecretKey::generate();
            std::fs::write(path, key.to_bytes())?;
            Ok(key)
        }
        Err(error) => Err(error.into()),
    }
}

fn timestamp(identity: &MembershipIdentity) -> Result<Hlc> {
    Ok(Hlc {
        physical_ms: now_ms()?,
        logical: 0,
        actor_id: ActorId::new(identity.fingerprint()),
    })
}

fn now_ms() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()
        .context("time does not fit u64")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn admission_sync_and_removal_are_enforced_over_iroh() -> Result<()> {
        let _guard = crate::iroh_node::IROH_TEST_LOCK.lock().await;
        let (relay_map, _relay_url, relay_server) = iroh::test_utils::run_relay_server().await?;
        let directory = tempfile::tempdir()?;
        let owner = AutomergeNode::persistent_with_relay_map(
            directory.path().join("owner"),
            PeerId::parse("owner")?,
            relay_map.clone(),
        )
        .await?;
        let phone = AutomergeNode::persistent_with_relay_map(
            directory.path().join("phone"),
            PeerId::parse("phone")?,
            relay_map,
        )
        .await?;
        let owner_workspace = owner.create_workspace().await?;
        let invitation = owner_workspace.invitation()?;

        assert_eq!(
            phone.request_join(&invitation).await?,
            JoinResponse::Pending
        );
        let pending = owner_workspace.pending_requests().await;
        assert_eq!(pending.len(), 1);
        owner_workspace.approve(&pending[0].public_key).await?;
        assert!(matches!(
            phone.request_join(&invitation).await?,
            JoinResponse::Approved { .. }
        ));

        let phone_workspace = phone.import_approved_workspace(&invitation).await?;
        owner_workspace
            .put("note/example", b"from owner".to_vec())
            .await?;
        phone_workspace.sync().await?;
        assert_eq!(
            phone_workspace.get("note/example").await?,
            Some(b"from owner".to_vec())
        );
        assert_eq!(phone_workspace.members().await.len(), 2);
        owner_workspace
            .put("note/gossip", b"automatic".to_vec())
            .await?;
        let mut discovered = false;
        for _ in 0..100 {
            if phone_workspace.get("note/gossip").await?.is_some() {
                discovered = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            discovered,
            "Gossip discovery did not trigger QUIC synchronization"
        );

        owner_workspace
            .remove(&phone.identity.public_key(), Some("lost".into()))
            .await?;
        assert!(phone_workspace.sync().await.is_err());
        owner.shutdown().await?;
        phone.shutdown().await?;
        drop(relay_server);
        Ok(())
    }
}
