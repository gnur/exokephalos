//! Transport-neutral control frames for centralized Automerge synchronization.
//!
//! WebSocket text frames use this module. After the hello exchange, binary frames
//! contain opaque Automerge sync messages and are bounded by [`MAX_SYNC_MESSAGE_BYTES`].

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const SYNC_PROTOCOL_VERSION: u16 = 1;
pub const MAX_CONTROL_MESSAGE_BYTES: usize = 16 * 1024;
pub const MAX_SYNC_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// Human-readable presence label. This is not a security identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ClientId(String);

impl ClientId {
    pub fn parse(value: impl Into<String>) -> Result<Self, SyncProtocolError> {
        let value = value.into();
        validate_client_id(&value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ClientId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    ClientHello {
        protocol_version: u16,
        client_id: String,
    },
    ServerHello {
        protocol_version: u16,
        workspace_id: String,
        clients: Vec<String>,
    },
    Presence {
        clients: Vec<String>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl ControlMessage {
    #[must_use]
    pub fn client_hello(client_id: impl Into<String>) -> Self {
        Self::ClientHello {
            protocol_version: SYNC_PROTOCOL_VERSION,
            client_id: client_id.into(),
        }
    }

    #[must_use]
    pub fn server_hello(
        workspace_id: impl Into<String>,
        clients: impl IntoIterator<Item = String>,
    ) -> Self {
        Self::ServerHello {
            protocol_version: SYNC_PROTOCOL_VERSION,
            workspace_id: workspace_id.into(),
            clients: clients.into_iter().collect(),
        }
    }

    pub fn encode(&self) -> Result<String, SyncProtocolError> {
        self.validate()?;
        let encoded = serde_json::to_string(self)?;
        if encoded.len() > MAX_CONTROL_MESSAGE_BYTES {
            return Err(SyncProtocolError::ControlTooLarge(encoded.len()));
        }
        Ok(encoded)
    }

    pub fn decode(encoded: &str) -> Result<Self, SyncProtocolError> {
        if encoded.len() > MAX_CONTROL_MESSAGE_BYTES {
            return Err(SyncProtocolError::ControlTooLarge(encoded.len()));
        }
        let message: Self = serde_json::from_str(encoded)?;
        message.validate()?;
        Ok(message)
    }

    pub fn validate(&self) -> Result<(), SyncProtocolError> {
        match self {
            Self::ClientHello {
                protocol_version,
                client_id,
            } => {
                validate_version(*protocol_version)?;
                validate_client_id(client_id)?;
            }
            Self::ServerHello {
                protocol_version,
                workspace_id,
                clients,
            } => {
                validate_version(*protocol_version)?;
                validate_workspace_id(workspace_id)?;
                for client in clients {
                    validate_client_id(client)?;
                }
            }
            Self::Presence { clients } => {
                for client in clients {
                    validate_client_id(client)?;
                }
            }
            Self::Error { code, message } => {
                if code.is_empty() || code.len() > 64 || !code.bytes().all(is_identifier_byte) {
                    return Err(SyncProtocolError::InvalidErrorCode);
                }
                if message.is_empty() || message.len() > 1024 {
                    return Err(SyncProtocolError::InvalidErrorMessage);
                }
            }
        }
        Ok(())
    }
}

pub fn validate_sync_message(bytes: &[u8]) -> Result<(), SyncProtocolError> {
    if bytes.is_empty() {
        return Err(SyncProtocolError::EmptySyncMessage);
    }
    if bytes.len() > MAX_SYNC_MESSAGE_BYTES {
        return Err(SyncProtocolError::SyncMessageTooLarge(bytes.len()));
    }
    Ok(())
}

fn validate_version(version: u16) -> Result<(), SyncProtocolError> {
    if version == SYNC_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(SyncProtocolError::UnsupportedVersion(version))
    }
}

fn validate_client_id(value: &str) -> Result<(), SyncProtocolError> {
    if value.is_empty() || value.len() > 64 || !value.bytes().all(is_identifier_byte) {
        return Err(SyncProtocolError::InvalidClientId);
    }
    Ok(())
}

fn validate_workspace_id(value: &str) -> Result<(), SyncProtocolError> {
    if value.is_empty() || value.len() > 128 || !value.bytes().all(is_identifier_byte) {
        return Err(SyncProtocolError::InvalidWorkspaceId);
    }
    Ok(())
}

const fn is_identifier_byte(value: u8) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, b'.' | b'_' | b'-')
}

#[derive(Debug, Error)]
pub enum SyncProtocolError {
    #[error("unsupported sync protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("client ID must contain 1-64 letters, digits, '.', '_', or '-'")]
    InvalidClientId,
    #[error("workspace ID must contain 1-128 letters, digits, '.', '_', or '-'")]
    InvalidWorkspaceId,
    #[error("error code is invalid")]
    InvalidErrorCode,
    #[error("error message is invalid")]
    InvalidErrorMessage,
    #[error("control message is {0} bytes and exceeds the limit")]
    ControlTooLarge(usize),
    #[error("Automerge sync message is empty")]
    EmptySyncMessage,
    #[error("Automerge sync message is {0} bytes and exceeds the limit")]
    SyncMessageTooLarge(usize),
    #[error("control message is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_frames_round_trip_and_are_versioned() {
        let hello = ControlMessage::client_hello("clever-browser");
        assert_eq!(
            ControlMessage::decode(&hello.encode().unwrap()).unwrap(),
            hello
        );
        let server = ControlMessage::server_hello(
            "workspace-1",
            ["clever-browser".to_owned(), "terminal".to_owned()],
        );
        assert_eq!(
            ControlMessage::decode(&server.encode().unwrap()).unwrap(),
            server
        );
    }

    #[test]
    fn invalid_identifiers_versions_and_frames_are_rejected() {
        assert!(
            ControlMessage::client_hello("contains spaces")
                .encode()
                .is_err()
        );
        assert!(
            ControlMessage::decode(
                r#"{"type":"client_hello","protocol_version":2,"client_id":"browser"}"#,
            )
            .is_err()
        );
        assert!(validate_sync_message(&[]).is_err());
        assert!(validate_sync_message(&vec![0; MAX_SYNC_MESSAGE_BYTES + 1]).is_err());
    }
}
