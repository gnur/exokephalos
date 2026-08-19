use std::collections::{BTreeMap, BTreeSet};
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
use xo_core::central_replica::CentralReplica;
use xo_core::central_sync::{ControlMessage, MAX_CONTROL_MESSAGE_BYTES};
use xo_core::domain::{Frontmatter, FrontmatterValue};
use xo_core::records::WorkspaceRecords;
use xo_core::{ActorId, CURRENT_SCHEMA, HlcClock, Note, NoteId, NoteRevision, RevisionId};

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
    replica: Arc<CentralReplica>,
    clock: Mutex<HlcClock>,
    mutation: Mutex<()>,
    clients: Mutex<BTreeMap<String, usize>>,
    notifications: broadcast::Sender<WorkspaceNotification>,
}

impl CentralWorkspace {
    pub fn open(state_dir: &Path) -> Result<Arc<Self>> {
        std::fs::create_dir_all(state_dir)
            .with_context(|| format!("create central workspace {}", state_dir.display()))?;
        let workspace_id = load_or_create_hex(&state_dir.join(WORKSPACE_ID_FILE), 16, "workspace")?;
        let actor_hex = load_or_create_hex(&state_dir.join(SERVER_ACTOR_FILE), 32, "actor")?;
        let automerge_actor = decode_hex(&actor_hex)?;
        let actor = ActorId::new(format!("server-{}", &actor_hex[..16]));
        let replica =
            CentralReplica::open(state_dir, &workspace_id, actor.clone(), &automerge_actor)?;
        let (notifications, _) = broadcast::channel(128);
        Ok(Arc::new(Self {
            workspace_id,
            replica,
            clock: Mutex::new(HlcClock::new(actor)),
            mutation: Mutex::new(()),
            clients: Mutex::new(BTreeMap::new()),
            notifications,
        }))
    }

    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    #[cfg(test)]
    pub async fn record(&self, key: &str) -> Result<Option<Vec<u8>>> {
        use xo_core::record_workspace::RecordWorkspace as _;
        self.replica.get_record(key).await
    }

    #[cfg(test)]
    pub async fn revision_count(&self, note_id: &NoteId) -> Result<usize> {
        Ok(WorkspaceRecords::new(self.replica.as_ref())
            .revision_history(note_id)
            .await?
            .len())
    }

    pub async fn create_item(&self, note: &Note) -> Result<bool> {
        let _mutation = self.mutation.lock().await;
        let records = WorkspaceRecords::new(self.replica.as_ref());
        if records.load_note(&note.id).await?.is_some() {
            return Ok(false);
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        let hlc = self.clock.lock().await.next(now);
        records
            .commit_revision(&NoteRevision {
                schema: CURRENT_SCHEMA,
                note_id: note.id.clone(),
                frontmatter: note.frontmatter.clone(),
                body: note.body.clone(),
                materialized_path: xo_core::projection::canonical_note_path(
                    &note.id,
                    &note.frontmatter,
                ),
                hlc,
                author_id: records.actor_id(),
                predecessors: BTreeSet::new(),
                deleted: false,
            })
            .await?;
        let _ = self
            .notifications
            .send(WorkspaceNotification::DocumentChanged);
        Ok(true)
    }

    pub async fn item(&self, note_id: &NoteId) -> Result<Option<Note>> {
        Ok(WorkspaceRecords::new(self.replica.as_ref())
            .load_note(note_id)
            .await?
            .and_then(|resolved| resolved.visible)
            .map(revision_note))
    }

    pub async fn patch_item(
        &self,
        note_id: &NoteId,
        frontmatter: Option<Frontmatter>,
        body: Option<String>,
    ) -> Result<Option<Note>> {
        let _mutation = self.mutation.lock().await;
        let records = WorkspaceRecords::new(self.replica.as_ref());
        let Some(resolved) = records.load_note(note_id).await? else {
            return Ok(None);
        };
        let Some(existing) = resolved.visible else {
            return Ok(None);
        };
        let note = Note {
            id: note_id.clone(),
            frontmatter: frontmatter.unwrap_or(existing.frontmatter),
            body: body.unwrap_or(existing.body),
            path: existing.materialized_path,
        };
        if let Some(value) = note.frontmatter.get("id")
            && value != &FrontmatterValue::String(note_id.to_string())
        {
            bail!("frontmatter id must match the item id");
        }
        self.commit_item(
            &records,
            &note,
            false,
            resolved.winning_revision,
            resolved.conflict,
        )
        .await?;
        Ok(Some(note))
    }

    pub async fn delete_item(&self, note_id: &NoteId) -> Result<bool> {
        let _mutation = self.mutation.lock().await;
        let records = WorkspaceRecords::new(self.replica.as_ref());
        let Some(resolved) = records.load_note(note_id).await? else {
            return Ok(false);
        };
        let Some(revision) = resolved.visible else {
            return Ok(false);
        };
        let note = revision_note(revision);
        self.commit_item(
            &records,
            &note,
            true,
            resolved.winning_revision,
            resolved.conflict,
        )
        .await?;
        Ok(true)
    }

    async fn commit_item(
        &self,
        records: &WorkspaceRecords<'_>,
        note: &Note,
        deleted: bool,
        winner: RevisionId,
        conflict: Option<xo_core::Conflict>,
    ) -> Result<()> {
        let mut predecessors = BTreeSet::from([winner]);
        predecessors.extend(
            conflict
                .into_iter()
                .flat_map(|value| value.concurrent_revisions),
        );
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        let hlc = self.clock.lock().await.next(now);
        records
            .commit_revision(&NoteRevision {
                schema: CURRENT_SCHEMA,
                note_id: note.id.clone(),
                frontmatter: note.frontmatter.clone(),
                body: note.body.clone(),
                materialized_path: xo_core::projection::canonical_note_path(
                    &note.id,
                    &note.frontmatter,
                ),
                hlc,
                author_id: records.actor_id(),
                predecessors,
                deleted,
            })
            .await?;
        let _ = self
            .notifications
            .send(WorkspaceNotification::DocumentChanged);
        Ok(())
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
            if let Some(message) = self.replica.generate_sync_message(&mut sync_state).await {
                sender.send(Message::Binary(message.into())).await?;
            }
            let mut notifications = self.notifications.subscribe();
            loop {
                tokio::select! {
                    incoming = receiver.next() => {
                        match incoming {
                            Some(Ok(Message::Binary(bytes))) => {
                                let changed = self.replica
                                    .receive_sync_message(&mut sync_state, &bytes)
                                    .await?;
                                if changed {
                                    let _ = self.notifications.send(
                                        WorkspaceNotification::DocumentChanged,
                                    );
                                }
                                if let Some(message) = self.replica
                                    .generate_sync_message(&mut sync_state).await
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
                                if let Some(message) = self.replica
                                    .generate_sync_message(&mut sync_state).await
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

fn revision_note(revision: NoteRevision) -> Note {
    Note {
        id: revision.note_id,
        frontmatter: revision.frontmatter,
        body: revision.body,
        path: revision.materialized_path,
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
