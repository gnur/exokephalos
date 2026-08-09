//! Peer identity and signed workspace-membership events.

use std::fmt;

use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Hlc, WorkspaceId};

const PEER_ID_MAX_LEN: usize = 64;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum MembershipError {
    #[error("peer ID must contain between 1 and {PEER_ID_MAX_LEN} characters")]
    InvalidPeerIdLength,
    #[error("peer ID may contain only ASCII letters, digits, '.', '_', and '-'")]
    InvalidPeerIdCharacter,
    #[error("membership event encoding failed: {0}")]
    Encoding(String),
    #[error("membership event ID does not match its contents")]
    EventIdMismatch,
    #[error("membership event issuer does not match its public key")]
    IssuerMismatch,
    #[error("membership event signature has an invalid length")]
    InvalidSignatureLength,
    #[error("membership event signature is invalid")]
    InvalidSignature,
    #[error("membership public key is invalid")]
    InvalidPublicKey,
}

/// A required, human-readable node identifier. Cryptographic identity uses the membership key.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PeerId(String);

impl PeerId {
    pub fn parse(value: impl Into<String>) -> Result<Self, MembershipError> {
        let value = value.into();
        let length = value.chars().count();
        if !(1..=PEER_ID_MAX_LEN).contains(&length) {
            return Err(MembershipError::InvalidPeerIdLength);
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(MembershipError::InvalidPeerIdCharacter);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A node-owned workspace membership identity, separate from its Iroh transport identity.
#[derive(Clone)]
pub struct MembershipIdentity {
    peer_id: PeerId,
    signing_key: SigningKey,
}

impl fmt::Debug for MembershipIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MembershipIdentity")
            .field("peer_id", &self.peer_id)
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

impl MembershipIdentity {
    #[must_use]
    pub fn generate(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            signing_key: SigningKey::from_bytes(&rand::random()),
        }
    }

    #[must_use]
    pub fn from_secret_bytes(peer_id: PeerId, bytes: &[u8; 32]) -> Self {
        Self {
            peer_id,
            signing_key: SigningKey::from_bytes(bytes),
        }
    }

    #[must_use]
    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    #[must_use]
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    #[must_use]
    pub fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    #[must_use]
    pub fn fingerprint(&self) -> String {
        public_key_fingerprint(&self.public_key())
    }

    #[must_use]
    pub fn actor_id(&self) -> crate::ActorId {
        crate::ActorId::new(self.fingerprint())
    }

    #[must_use]
    pub fn sign(&self, bytes: &[u8]) -> Vec<u8> {
        self.signing_key.sign(bytes).to_bytes().to_vec()
    }
}

#[must_use]
pub fn public_key_fingerprint(public_key: &[u8; 32]) -> String {
    blake3::hash(public_key).to_hex().to_string()
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MembershipEvent {
    Genesis {
        peer_id: PeerId,
        public_key: [u8; 32],
        endpoint_id: String,
    },
    JoinApproved {
        peer_id: PeerId,
        public_key: [u8; 32],
        endpoint_id: String,
    },
    JoinRejected {
        peer_id: PeerId,
        public_key: [u8; 32],
    },
    EndpointBound {
        public_key: [u8; 32],
        endpoint_id: String,
    },
    PeerRemoved {
        peer_id: PeerId,
        public_key: [u8; 32],
        accepted_actor_sequence: u64,
        accepted_heads: Vec<String>,
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct UnsignedMembershipEvent {
    workspace_id: WorkspaceId,
    issuer: String,
    issued_at: Hlc,
    payload: MembershipEvent,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SignedMembershipEvent {
    pub workspace_id: WorkspaceId,
    pub event_id: String,
    pub issuer: String,
    pub issuer_public_key: [u8; 32],
    pub issued_at: Hlc,
    pub payload: MembershipEvent,
    pub signature: Vec<u8>,
}

impl SignedMembershipEvent {
    pub fn create(
        identity: &MembershipIdentity,
        workspace_id: WorkspaceId,
        issued_at: Hlc,
        payload: MembershipEvent,
    ) -> Result<Self, MembershipError> {
        let issuer = identity.fingerprint();
        let unsigned = UnsignedMembershipEvent {
            workspace_id: workspace_id.clone(),
            issuer: issuer.clone(),
            issued_at: issued_at.clone(),
            payload: payload.clone(),
        };
        let bytes = canonical_bytes(&unsigned)?;
        Ok(Self {
            workspace_id,
            event_id: blake3::hash(&bytes).to_hex().to_string(),
            issuer,
            issuer_public_key: identity.public_key(),
            issued_at,
            payload,
            signature: identity.sign(&bytes),
        })
    }

    pub fn verify(&self) -> Result<(), MembershipError> {
        if self.issuer != public_key_fingerprint(&self.issuer_public_key) {
            return Err(MembershipError::IssuerMismatch);
        }
        let unsigned = UnsignedMembershipEvent {
            workspace_id: self.workspace_id.clone(),
            issuer: self.issuer.clone(),
            issued_at: self.issued_at.clone(),
            payload: self.payload.clone(),
        };
        let bytes = canonical_bytes(&unsigned)?;
        if self.event_id != blake3::hash(&bytes).to_hex().to_string() {
            return Err(MembershipError::EventIdMismatch);
        }
        let key = VerifyingKey::from_bytes(&self.issuer_public_key)
            .map_err(|_| MembershipError::InvalidPublicKey)?;
        let signature_bytes: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| MembershipError::InvalidSignatureLength)?;
        key.verify(&bytes, &Signature::from_bytes(&signature_bytes))
            .map_err(|_| MembershipError::InvalidSignature)
    }
}

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, MembershipError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes)
        .map_err(|error| MembershipError::Encoding(error.to_string()))?;
    Ok(bytes)
}

#[cfg(feature = "native")]
pub fn load_or_create_identity(
    state_dir: &std::path::Path,
    requested_peer_id: &PeerId,
) -> anyhow::Result<MembershipIdentity> {
    use anyhow::{Context as _, bail};
    use std::io::Write as _;

    let identity_dir = state_dir.join("identity");
    std::fs::create_dir_all(&identity_dir)
        .with_context(|| format!("create {}", identity_dir.display()))?;
    let peer_id_path = identity_dir.join("peer-id");
    let key_path = identity_dir.join("membership.key");

    if let Ok(saved) = std::fs::read_to_string(&peer_id_path) {
        let saved = PeerId::parse(saved.trim()).context("validate saved peer ID")?;
        if &saved != requested_peer_id {
            bail!(
                "state belongs to peer ID {saved:?}, not requested peer ID {requested_peer_id:?}"
            );
        }
    }

    let identity = match std::fs::read(&key_path) {
        Ok(bytes) => {
            let secret: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("membership key must contain exactly 32 bytes"))?;
            MembershipIdentity::from_secret_bytes(requested_peer_id.clone(), &secret)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let identity = MembershipIdentity::generate(requested_peer_id.clone());
            let temporary = identity_dir.join("membership.key.tmp");
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temporary)
                .with_context(|| format!("create {}", temporary.display()))?;
            file.write_all(&identity.secret_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temporary, &key_path)
                .with_context(|| format!("install {}", key_path.display()))?;
            identity
        }
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", key_path.display()));
        }
    };

    if !peer_id_path.exists() {
        std::fs::write(&peer_id_path, format!("{requested_peer_id}\n"))
            .with_context(|| format!("write {}", peer_id_path.display()))?;
    }
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ActorId;

    fn time(actor: &str) -> Hlc {
        Hlc {
            physical_ms: 10,
            logical: 0,
            actor_id: ActorId::new(actor),
        }
    }

    #[test]
    fn validates_peer_ids() {
        assert_eq!(
            PeerId::parse("workstation-1").unwrap().as_str(),
            "workstation-1"
        );
        for invalid in ["", "contains spaces", "phone/one"] {
            assert!(PeerId::parse(invalid).is_err());
        }
    }

    #[test]
    fn membership_events_are_signed_and_tamper_evident() {
        let identity = MembershipIdentity::generate(PeerId::parse("laptop").unwrap());
        let actor = identity.actor_id();
        let mut event = SignedMembershipEvent::create(
            &identity,
            WorkspaceId::new("workspace"),
            time(actor.as_str()),
            MembershipEvent::Genesis {
                peer_id: identity.peer_id().clone(),
                public_key: identity.public_key(),
                endpoint_id: "endpoint-a".to_owned(),
            },
        )
        .unwrap();
        event.verify().unwrap();
        event.payload = MembershipEvent::JoinRejected {
            peer_id: PeerId::parse("other").unwrap(),
            public_key: [3; 32],
        };
        assert_eq!(
            event.verify().unwrap_err(),
            MembershipError::EventIdMismatch
        );
    }

    #[test]
    fn identities_round_trip_secret_bytes() {
        let identity = MembershipIdentity::generate(PeerId::parse("phone").unwrap());
        let restored = MembershipIdentity::from_secret_bytes(
            identity.peer_id().clone(),
            &identity.secret_bytes(),
        );
        assert_eq!(restored.public_key(), identity.public_key());
        assert_eq!(restored.fingerprint(), identity.fingerprint());
    }
}
