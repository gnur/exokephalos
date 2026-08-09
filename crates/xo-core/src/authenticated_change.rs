//! Ed25519 authentication envelopes for changes forwarded between workspace members.

use automerge::Change;
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::MembershipIdentity;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct ChangeSignaturePayload {
    workspace_id: String,
    public_key: [u8; 32],
    sequence: u64,
    change_hash: String,
    change_bytes_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SignedAutomergeChange {
    pub workspace_id: String,
    pub public_key: [u8; 32],
    pub sequence: u64,
    pub change_hash: String,
    pub change_bytes: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum AuthenticatedChangeError {
    #[error("change actor does not match the membership public key")]
    ActorMismatch,
    #[error("change sequence does not match its signed envelope")]
    SequenceMismatch,
    #[error("change hash does not match its signed envelope")]
    HashMismatch,
    #[error("change signature has an invalid length")]
    InvalidSignatureLength,
    #[error("change signature is invalid")]
    InvalidSignature,
    #[error("Automerge change is invalid: {0}")]
    InvalidChange(String),
    #[error("change signature payload encoding failed: {0}")]
    Encoding(String),
}

impl SignedAutomergeChange {
    pub fn create(
        workspace_id: impl Into<String>,
        identity: &MembershipIdentity,
        change: &Change,
    ) -> Result<Self, AuthenticatedChangeError> {
        if change.actor_id().to_bytes() != identity.public_key() {
            return Err(AuthenticatedChangeError::ActorMismatch);
        }
        let workspace_id = workspace_id.into();
        let change_bytes = change.raw_bytes().to_vec();
        let payload = ChangeSignaturePayload {
            workspace_id: workspace_id.clone(),
            public_key: identity.public_key(),
            sequence: change.seq(),
            change_hash: change.hash().to_string(),
            change_bytes_hash: blake3::hash(&change_bytes).to_hex().to_string(),
        };
        let encoded = encode(&payload)?;
        Ok(Self {
            workspace_id,
            public_key: payload.public_key,
            sequence: payload.sequence,
            change_hash: payload.change_hash,
            change_bytes,
            signature: identity.sign(&encoded),
        })
    }

    pub fn verify(&self) -> Result<Change, AuthenticatedChangeError> {
        let change = Change::from_bytes(self.change_bytes.clone())
            .map_err(|error| AuthenticatedChangeError::InvalidChange(error.to_string()))?;
        if change.actor_id().to_bytes() != self.public_key {
            return Err(AuthenticatedChangeError::ActorMismatch);
        }
        if change.seq() != self.sequence {
            return Err(AuthenticatedChangeError::SequenceMismatch);
        }
        if change.hash().to_string() != self.change_hash {
            return Err(AuthenticatedChangeError::HashMismatch);
        }
        let payload = ChangeSignaturePayload {
            workspace_id: self.workspace_id.clone(),
            public_key: self.public_key,
            sequence: self.sequence,
            change_hash: self.change_hash.clone(),
            change_bytes_hash: blake3::hash(&self.change_bytes).to_hex().to_string(),
        };
        let encoded = encode(&payload)?;
        let key = VerifyingKey::from_bytes(&self.public_key)
            .map_err(|_| AuthenticatedChangeError::InvalidSignature)?;
        let signature: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| AuthenticatedChangeError::InvalidSignatureLength)?;
        key.verify(&encoded, &Signature::from_bytes(&signature))
            .map_err(|_| AuthenticatedChangeError::InvalidSignature)?;
        Ok(change)
    }
}

fn encode(value: &ChangeSignaturePayload) -> Result<Vec<u8>, AuthenticatedChangeError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes)
        .map_err(|error| AuthenticatedChangeError::Encoding(error.to_string()))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use automerge::transaction::Transactable as _;
    use automerge::{ActorId, AutoCommit, ROOT};

    use super::*;
    use crate::PeerId;

    #[test]
    fn signed_changes_bind_workspace_actor_sequence_and_bytes() {
        let identity = MembershipIdentity::generate(PeerId::parse("laptop").unwrap());
        let mut document =
            AutoCommit::new().with_actor(ActorId::from(identity.public_key().as_slice()));
        document.put(ROOT, "note/a", vec![1_u8]).unwrap();
        document.commit();
        let change = document.get_last_local_change().unwrap();
        let signed = SignedAutomergeChange::create("workspace-a", &identity, change).unwrap();
        signed.verify().unwrap();

        let mut damaged = signed;
        damaged.change_bytes.push(1);
        assert!(damaged.verify().is_err());
    }
}
