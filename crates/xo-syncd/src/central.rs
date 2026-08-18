use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use automerge::sync::State as SyncState;
use futures_util::{SinkExt as _, StreamExt as _};
use rand::RngCore as _;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, broadcast};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use xo_core::automerge_store::PersistentAutomergeStore;
use xo_core::central_sync::{ControlMessage, MAX_CONTROL_MESSAGE_BYTES};

const WORKSPACE_ID_FILE: &str = "workspace-id";
const SERVER_ACTOR_FILE: &str = "server-actor";

#[derive(Clone, Copy, Debug)]
enum WorkspaceNotification {
    DocumentChanged,
    PresenceChanged,
}

#[derive(Debug)]
pub struct CentralWorkspace {
    workspace_id: String,
    store: Mutex<PersistentAutomergeStore>,
    clients: Mutex<BTreeMap<String, usize>>,
    notifications: broadcast::Sender<WorkspaceNotification>,
}

impl CentralWorkspace {
    pub fn open(state_dir: &Path) -> Result<Arc<Self>> {
        std::fs::create_dir_all(state_dir)
            .with_context(|| format!("create central workspace {}", state_dir.display()))?;
        let workspace_id = load_or_create_hex(&state_dir.join(WORKSPACE_ID_FILE), 16, "workspace")?;
        let actor_hex = load_or_create_hex(&state_dir.join(SERVER_ACTOR_FILE), 32, "actor")?;
        let actor = decode_hex(&actor_hex)?;
        let store = PersistentAutomergeStore::open_or_create(
            &state_dir.join("replica"),
            &workspace_id,
            &actor,
        )?;
        let (notifications, _) = broadcast::channel(128);
        Ok(Arc::new(Self {
            workspace_id,
            store: Mutex::new(store),
            clients: Mutex::new(BTreeMap::new()),
            notifications,
        }))
    }

    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    async fn client_ids(&self) -> Vec<String> {
        self.clients.lock().await.keys().cloned().collect()
    }

    async fn add_client(&self, client_id: &str) {
        let mut clients = self.clients.lock().await;
        *clients.entry(client_id.to_owned()).or_default() += 1;
        drop(clients);
        let _ = self
            .notifications
            .send(WorkspaceNotification::PresenceChanged);
    }

    async fn remove_client(&self, client_id: &str) {
        let mut clients = self.clients.lock().await;
        if let Some(count) = clients.get_mut(client_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                clients.remove(client_id);
            }
        }
        drop(clients);
        let _ = self
            .notifications
            .send(WorkspaceNotification::PresenceChanged);
    }

    pub async fn serve_socket<S>(&self, socket: WebSocketStream<S>) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let (mut sender, mut receiver) = socket.split();
        let hello = tokio::time::timeout(std::time::Duration::from_secs(10), receiver.next())
            .await
            .context("client hello timed out")?
            .context("client disconnected before hello")??;
        let Message::Text(encoded) = hello else {
            bail!("first WebSocket message must be a client_hello text frame");
        };
        if encoded.len() > MAX_CONTROL_MESSAGE_BYTES {
            bail!("client hello exceeds control message limit");
        }
        let ControlMessage::ClientHello { client_id, .. } = ControlMessage::decode(&encoded)?
        else {
            bail!("first WebSocket message must be client_hello");
        };

        self.add_client(&client_id).await;
        let result = async {
            sender
                .send(Message::Text(
                    ControlMessage::server_hello(
                        self.workspace_id().to_owned(),
                        self.client_ids().await,
                    )
                    .encode()?
                    .into(),
                ))
                .await?;
            let mut sync_state = SyncState::new();
            if let Some(message) = self
                .store
                .lock()
                .await
                .generate_sync_message(&mut sync_state)
            {
                sender.send(Message::Binary(message.into())).await?;
            }
            let mut notifications = self.notifications.subscribe();
            loop {
                tokio::select! {
                    incoming = receiver.next() => {
                        match incoming {
                            Some(Ok(Message::Binary(bytes))) => {
                                let changed = self.store.lock().await.receive_sync_message(
                                    &mut sync_state,
                                    &bytes,
                                )?;
                                if changed {
                                    let _ = self.notifications.send(
                                        WorkspaceNotification::DocumentChanged,
                                    );
                                }
                                if let Some(message) = self.store.lock().await
                                    .generate_sync_message(&mut sync_state)
                                {
                                    sender.send(Message::Binary(message.into())).await?;
                                }
                            }
                            Some(Ok(Message::Ping(bytes))) => {
                                sender.send(Message::Pong(bytes)).await?;
                            }
                            Some(Ok(Message::Close(_))) | None => break,
                            Some(Ok(Message::Text(_)|Message::Pong(_)|Message::Frame(_))) => {}
                            Some(Err(error)) => return Err(error.into()),
                        }
                    }
                    notification = notifications.recv() => {
                        match notification {
                            Ok(WorkspaceNotification::DocumentChanged) => {
                                if let Some(message) = self.store.lock().await
                                    .generate_sync_message(&mut sync_state)
                                {
                                    sender.send(Message::Binary(message.into())).await?;
                                }
                            }
                            Ok(WorkspaceNotification::PresenceChanged) => {
                                sender.send(Message::Text(ControlMessage::Presence {
                                    clients: self.client_ids().await,
                                }.encode()?.into())).await?;
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {},
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
            Ok(())
        }
        .await;
        self.remove_client(&client_id).await;
        result
    }
}

fn load_or_create_hex(path: &Path, random_bytes: usize, prefix: &str) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(value) => {
            let value = value.trim();
            if prefix == "actor" {
                decode_hex(value)?;
            } else {
                ControlMessage::server_hello(value, []).validate()?;
            }
            Ok(value.to_owned())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut random = vec![0; random_bytes];
            rand::rng().fill_bytes(&mut random);
            let value = format!("{prefix}-{}", encode_hex(&random));
            if prefix == "actor" {
                std::fs::write(path, format!("{}\n", encode_hex(&random)))?;
                Ok(encode_hex(&random))
            } else {
                std::fs::write(path, format!("{value}\n"))?;
                Ok(value)
            }
        }
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("string writes cannot fail");
    }
    output
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    if value.len() % 2 != 0 {
        bail!("hex value has an odd length");
    }
    (0..value.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).context("invalid hex"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn central_workspace_identity_is_durable() {
        let directory = tempfile::tempdir().unwrap();
        let first = CentralWorkspace::open(directory.path()).unwrap();
        let workspace_id = first.workspace_id().to_owned();
        drop(first);
        let second = CentralWorkspace::open(directory.path()).unwrap();
        assert_eq!(second.workspace_id(), workspace_id);
    }
}
