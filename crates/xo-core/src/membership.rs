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
    #[error("membership event issuer is not authorized")]
    UnauthorizedEvent,
    #[error("membership event references an unknown member")]
    UnknownMember,
    #[error("membership public key is already registered")]
    DuplicateMember,
    #[error("peer ID is already used by an active member")]
    DuplicatePeerId,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemberStatus {
    Active,
    Rejected,
    Removed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Member {
    pub peer_id: PeerId,
    pub public_key: [u8; 32],
    pub endpoint_ids: std::collections::BTreeSet<String>,
    pub status: MemberStatus,
    pub accepted_actor_sequence: Option<u64>,
    pub accepted_heads: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct MembershipRegistry {
    members: std::collections::BTreeMap<String, Member>,
    peer_ids: std::collections::BTreeMap<PeerId, String>,
    events: std::collections::BTreeSet<String>,
}

impl MembershipRegistry {
    pub fn apply(&mut self, event: &SignedMembershipEvent) -> Result<bool, MembershipError> {
        event.verify()?;
        if self.events.contains(&event.event_id) {
            return Ok(false);
        }
        let issuer_active = self
            .members
            .get(&event.issuer)
            .is_some_and(|member| member.status == MemberStatus::Active);
        match &event.payload {
            MembershipEvent::Genesis {
                peer_id,
                public_key,
                endpoint_id,
            } => {
                if !self.members.is_empty()
                    || *public_key != event.issuer_public_key
                    || public_key_fingerprint(public_key) != event.issuer
                {
                    return Err(MembershipError::UnauthorizedEvent);
                }
                self.insert_member(peer_id, public_key, endpoint_id, MemberStatus::Active)?;
            }
            MembershipEvent::JoinApproved {
                peer_id,
                public_key,
                endpoint_id,
            } => {
                if !issuer_active {
                    return Err(MembershipError::UnauthorizedEvent);
                }
                self.insert_member(peer_id, public_key, endpoint_id, MemberStatus::Active)?;
            }
            MembershipEvent::JoinRejected {
                peer_id,
                public_key,
            } => {
                if !issuer_active {
                    return Err(MembershipError::UnauthorizedEvent);
                }
                self.insert_member(peer_id, public_key, "", MemberStatus::Rejected)?;
            }
            MembershipEvent::EndpointBound {
                public_key,
                endpoint_id,
            } => {
                let fingerprint = public_key_fingerprint(public_key);
                if event.issuer != fingerprint {
                    return Err(MembershipError::UnauthorizedEvent);
                }
                let member = self
                    .members
                    .get_mut(&fingerprint)
                    .filter(|member| member.status == MemberStatus::Active)
                    .ok_or(MembershipError::UnknownMember)?;
                member.endpoint_ids.insert(endpoint_id.clone());
            }
            MembershipEvent::PeerRemoved {
                peer_id,
                public_key,
                accepted_actor_sequence,
                accepted_heads,
                ..
            } => {
                if !issuer_active {
                    return Err(MembershipError::UnauthorizedEvent);
                }
                let fingerprint = public_key_fingerprint(public_key);
                let member = self
                    .members
                    .get_mut(&fingerprint)
                    .filter(|member| member.peer_id == *peer_id)
                    .ok_or(MembershipError::UnknownMember)?;
                member.status = MemberStatus::Removed;
                member.accepted_actor_sequence = Some(*accepted_actor_sequence);
                member.accepted_heads.clone_from(accepted_heads);
                self.peer_ids.remove(peer_id);
            }
        }
        self.events.insert(event.event_id.clone());
        Ok(true)
    }

    fn insert_member(
        &mut self,
        peer_id: &PeerId,
        public_key: &[u8; 32],
        endpoint_id: &str,
        status: MemberStatus,
    ) -> Result<(), MembershipError> {
        let fingerprint = public_key_fingerprint(public_key);
        if self.members.contains_key(&fingerprint) {
            return Err(MembershipError::DuplicateMember);
        }
        if status == MemberStatus::Active && self.peer_ids.contains_key(peer_id) {
            return Err(MembershipError::DuplicatePeerId);
        }
        let endpoint_ids = if endpoint_id.is_empty() {
            std::collections::BTreeSet::new()
        } else {
            std::collections::BTreeSet::from([endpoint_id.to_owned()])
        };
        self.members.insert(
            fingerprint.clone(),
            Member {
                peer_id: peer_id.clone(),
                public_key: *public_key,
                endpoint_ids,
                status,
                accepted_actor_sequence: None,
                accepted_heads: Vec::new(),
            },
        );
        if status == MemberStatus::Active {
            self.peer_ids.insert(peer_id.clone(), fingerprint);
        }
        Ok(())
    }

    #[must_use]
    pub fn member(&self, fingerprint: &str) -> Option<&Member> {
        self.members.get(fingerprint)
    }

    pub fn members(&self) -> impl Iterator<Item = &Member> {
        self.members.values()
    }

    #[must_use]
    pub fn is_active_key(&self, public_key: &[u8; 32]) -> bool {
        self.member(&public_key_fingerprint(public_key))
            .is_some_and(|member| member.status == MemberStatus::Active)
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

    #[test]
    fn registry_propagates_approval_and_terminal_removal() {
        let owner = MembershipIdentity::generate(PeerId::parse("owner").unwrap());
        let candidate = MembershipIdentity::generate(PeerId::parse("phone").unwrap());
        let workspace = WorkspaceId::new("workspace");
        let genesis = SignedMembershipEvent::create(
            &owner,
            workspace.clone(),
            time(owner.actor_id().as_str()),
            MembershipEvent::Genesis {
                peer_id: owner.peer_id().clone(),
                public_key: owner.public_key(),
                endpoint_id: "owner-endpoint".into(),
            },
        )
        .unwrap();
        let approval = SignedMembershipEvent::create(
            &owner,
            workspace.clone(),
            time(owner.actor_id().as_str()),
            MembershipEvent::JoinApproved {
                peer_id: candidate.peer_id().clone(),
                public_key: candidate.public_key(),
                endpoint_id: "phone-endpoint".into(),
            },
        )
        .unwrap();
        let removal = SignedMembershipEvent::create(
            &owner,
            workspace,
            time(owner.actor_id().as_str()),
            MembershipEvent::PeerRemoved {
                peer_id: candidate.peer_id().clone(),
                public_key: candidate.public_key(),
                accepted_actor_sequence: 4,
                accepted_heads: vec!["head".into()],
                reason: None,
            },
        )
        .unwrap();

        let mut registry = MembershipRegistry::default();
        registry.apply(&genesis).unwrap();
        registry.apply(&approval).unwrap();
        assert!(registry.is_active_key(&candidate.public_key()));
        registry.apply(&removal).unwrap();
        assert!(!registry.is_active_key(&candidate.public_key()));
        assert_eq!(
            registry.member(&candidate.fingerprint()).unwrap().status,
            MemberStatus::Removed
        );
    }

    #[test]
    fn unknown_key_cannot_approve_a_candidate() {
        let owner = MembershipIdentity::generate(PeerId::parse("owner").unwrap());
        let outsider = MembershipIdentity::generate(PeerId::parse("outsider").unwrap());
        let candidate = MembershipIdentity::generate(PeerId::parse("phone").unwrap());
        let workspace = WorkspaceId::new("workspace");
        let genesis = SignedMembershipEvent::create(
            &owner,
            workspace.clone(),
            time(owner.actor_id().as_str()),
            MembershipEvent::Genesis {
                peer_id: owner.peer_id().clone(),
                public_key: owner.public_key(),
                endpoint_id: "owner-endpoint".into(),
            },
        )
        .unwrap();
        let forged = SignedMembershipEvent::create(
            &outsider,
            workspace,
            time(outsider.actor_id().as_str()),
            MembershipEvent::JoinApproved {
                peer_id: candidate.peer_id().clone(),
                public_key: candidate.public_key(),
                endpoint_id: "phone-endpoint".into(),
            },
        )
        .unwrap();
        let mut registry = MembershipRegistry::default();
        registry.apply(&genesis).unwrap();
        assert_eq!(
            registry.apply(&forged).unwrap_err(),
            MembershipError::UnauthorizedEvent
        );
    }
}
