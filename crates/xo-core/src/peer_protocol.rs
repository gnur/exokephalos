//! Versioned admission and authenticated synchronization messages.

use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{MembershipIdentity, PeerId};

pub const JOIN_ALPN: &[u8] = b"/xo/join/1";
pub const AUTOMERGE_ALPN: &[u8] = b"/xo/automerge/1";
pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_PROTOCOL_FRAME: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceInvitation {
    pub version: u16,
    pub workspace_id: String,
    pub bootstrap_peers: Vec<iroh::EndpointAddr>,
    pub gossip_topic: [u8; 32],
    pub base_gossip_topic: [u8; 32],
    pub membership_epoch: u64,
    pub genesis_key_fingerprint: String,
}

impl WorkspaceInvitation {
    pub fn encode(&self) -> Result<String, ProtocolError> {
        if self.version != PROTOCOL_VERSION || self.bootstrap_peers.is_empty() {
            return Err(ProtocolError::InvalidInvitation);
        }
        let bytes = encode(self)?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn decode(value: &str) -> Result<Self, ProtocolError> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value.trim())
            .map_err(|_| ProtocolError::InvalidInvitation)?;
        let invitation: Self = decode(&bytes)?;
        if invitation.version != PROTOCOL_VERSION
            || invitation.workspace_id.is_empty()
            || invitation.bootstrap_peers.is_empty()
        {
            return Err(ProtocolError::InvalidInvitation);
        }
        Ok(invitation)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct JoinRequest {
    pub version: u16,
    pub workspace_id: String,
    pub peer_id: PeerId,
    pub public_key: [u8; 32],
    pub endpoint_id: String,
    pub endpoint_addresses: Vec<String>,
    pub requested_at_ms: u64,
    pub nonce: [u8; 32],
    pub signature: Vec<u8>,
}

#[derive(Serialize)]
struct JoinRequestPayload<'a> {
    version: u16,
    workspace_id: &'a str,
    peer_id: &'a PeerId,
    public_key: [u8; 32],
    endpoint_id: &'a str,
    endpoint_addresses: &'a [String],
    requested_at_ms: u64,
    nonce: [u8; 32],
}

impl JoinRequest {
    pub fn create(
        identity: &MembershipIdentity,
        workspace_id: impl Into<String>,
        endpoint_id: impl Into<String>,
        endpoint_addresses: Vec<String>,
        requested_at_ms: u64,
    ) -> Result<Self, ProtocolError> {
        let mut request = Self {
            version: PROTOCOL_VERSION,
            workspace_id: workspace_id.into(),
            peer_id: identity.peer_id().clone(),
            public_key: identity.public_key(),
            endpoint_id: endpoint_id.into(),
            endpoint_addresses,
            requested_at_ms,
            nonce: rand::random(),
            signature: Vec::new(),
        };
        request.signature = identity.sign(&request.payload_bytes()?);
        Ok(request)
    }

    pub fn verify(&self) -> Result<(), ProtocolError> {
        if self.version != PROTOCOL_VERSION || self.workspace_id.is_empty() {
            return Err(ProtocolError::InvalidJoinRequest);
        }
        verify_signature(&self.public_key, &self.payload_bytes()?, &self.signature)
    }

    fn payload_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        encode(&JoinRequestPayload {
            version: self.version,
            workspace_id: &self.workspace_id,
            peer_id: &self.peer_id,
            public_key: self.public_key,
            endpoint_id: &self.endpoint_id,
            endpoint_addresses: &self.endpoint_addresses,
            requested_at_ms: self.requested_at_ms,
            nonce: self.nonce,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum JoinResponse {
    Pending,
    Approved { membership_event: Vec<u8> },
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthHello {
    pub version: u16,
    pub workspace_id: String,
    pub peer_id: PeerId,
    pub public_key: [u8; 32],
    pub endpoint_id: String,
    pub nonce: [u8; 32],
    pub remote_nonce: [u8; 32],
    pub signature: Vec<u8>,
}

#[derive(Serialize)]
struct AuthPayload<'a> {
    version: u16,
    workspace_id: &'a str,
    peer_id: &'a PeerId,
    public_key: [u8; 32],
    endpoint_id: &'a str,
    nonce: [u8; 32],
    remote_nonce: [u8; 32],
}

impl AuthHello {
    pub fn create(
        identity: &MembershipIdentity,
        workspace_id: impl Into<String>,
        endpoint_id: impl Into<String>,
        remote_nonce: [u8; 32],
    ) -> Result<Self, ProtocolError> {
        let mut hello = Self {
            version: PROTOCOL_VERSION,
            workspace_id: workspace_id.into(),
            peer_id: identity.peer_id().clone(),
            public_key: identity.public_key(),
            endpoint_id: endpoint_id.into(),
            nonce: rand::random(),
            remote_nonce,
            signature: Vec::new(),
        };
        hello.signature = identity.sign(&hello.payload_bytes()?);
        Ok(hello)
    }

    pub fn verify(&self) -> Result<(), ProtocolError> {
        if self.version != PROTOCOL_VERSION || self.workspace_id.is_empty() {
            return Err(ProtocolError::InvalidAuthHello);
        }
        verify_signature(&self.public_key, &self.payload_bytes()?, &self.signature)
    }

    fn payload_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        encode(&AuthPayload {
            version: self.version,
            workspace_id: &self.workspace_id,
            peer_id: &self.peer_id,
            public_key: self.public_key,
            endpoint_id: &self.endpoint_id,
            nonce: self.nonce,
            remote_nonce: self.remote_nonce,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GossipAnnouncement {
    pub version: u16,
    pub workspace_id: String,
    pub peer_id: PeerId,
    pub public_key: [u8; 32],
    pub endpoint_id: String,
    pub heads: Vec<String>,
    pub timestamp_ms: u64,
    pub signature: Vec<u8>,
}

#[derive(Serialize)]
struct GossipAnnouncementPayload<'a> {
    version: u16,
    workspace_id: &'a str,
    peer_id: &'a PeerId,
    public_key: [u8; 32],
    endpoint_id: &'a str,
    heads: &'a [String],
    timestamp_ms: u64,
}

impl GossipAnnouncement {
    pub fn create(
        identity: &MembershipIdentity,
        workspace_id: impl Into<String>,
        endpoint_id: impl Into<String>,
        heads: Vec<String>,
        timestamp_ms: u64,
    ) -> Result<Self, ProtocolError> {
        let mut announcement = Self {
            version: PROTOCOL_VERSION,
            workspace_id: workspace_id.into(),
            peer_id: identity.peer_id().clone(),
            public_key: identity.public_key(),
            endpoint_id: endpoint_id.into(),
            heads,
            timestamp_ms,
            signature: Vec::new(),
        };
        announcement.signature = identity.sign(&announcement.payload_bytes()?);
        Ok(announcement)
    }

    pub fn verify(&self) -> Result<(), ProtocolError> {
        if self.version != PROTOCOL_VERSION || self.workspace_id.is_empty() {
            return Err(ProtocolError::InvalidAnnouncement);
        }
        verify_signature(&self.public_key, &self.payload_bytes()?, &self.signature)
    }

    fn payload_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        encode(&GossipAnnouncementPayload {
            version: self.version,
            workspace_id: &self.workspace_id,
            peer_id: &self.peer_id,
            public_key: self.public_key,
            endpoint_id: &self.endpoint_id,
            heads: &self.heads,
            timestamp_ms: self.timestamp_ms,
        })
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ProtocolError {
    #[error("workspace invitation is invalid")]
    InvalidInvitation,
    #[error("join request is invalid")]
    InvalidJoinRequest,
    #[error("authentication hello is invalid")]
    InvalidAuthHello,
    #[error("Gossip announcement is invalid")]
    InvalidAnnouncement,
    #[error("protocol message encoding failed")]
    Encoding,
    #[error("protocol message signature has an invalid length")]
    InvalidSignatureLength,
    #[error("protocol message public key is invalid")]
    InvalidPublicKey,
    #[error("protocol message signature is invalid")]
    InvalidSignature,
}

fn verify_signature(
    public_key: &[u8; 32],
    payload: &[u8],
    signature: &[u8],
) -> Result<(), ProtocolError> {
    let key = VerifyingKey::from_bytes(public_key).map_err(|_| ProtocolError::InvalidPublicKey)?;
    let signature: [u8; 64] = signature
        .try_into()
        .map_err(|_| ProtocolError::InvalidSignatureLength)?;
    key.verify(payload, &Signature::from_bytes(&signature))
        .map_err(|_| ProtocolError::InvalidSignature)
}

pub fn encode(value: &impl Serialize) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).map_err(|_| ProtocolError::Encoding)?;
    Ok(bytes)
}

pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ProtocolError> {
    ciborium::from_reader(bytes).map_err(|_| ProtocolError::Encoding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invitations_and_join_requests_round_trip() {
        let invitation = WorkspaceInvitation {
            version: PROTOCOL_VERSION,
            workspace_id: "workspace".into(),
            bootstrap_peers: vec![iroh::EndpointAddr::new(
                iroh::SecretKey::generate().public(),
            )],
            gossip_topic: [7; 32],
            base_gossip_topic: [7; 32],
            membership_epoch: 0,
            genesis_key_fingerprint: "genesis".into(),
        };
        assert_eq!(
            WorkspaceInvitation::decode(&invitation.encode().unwrap()).unwrap(),
            invitation
        );

        let identity = MembershipIdentity::generate(PeerId::parse("phone").unwrap());
        let request =
            JoinRequest::create(&identity, "workspace", "endpoint", Vec::new(), 10).unwrap();
        request.verify().unwrap();
        let mut damaged = request;
        damaged.peer_id = PeerId::parse("attacker").unwrap();
        assert_eq!(
            damaged.verify().unwrap_err(),
            ProtocolError::InvalidSignature
        );
    }
}
