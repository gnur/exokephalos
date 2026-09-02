use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use automerge::sync::State as SyncState;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use url::Url;
use xo_core::central_replica::{CentralReplica, ReplicaEvent};
use xo_core::central_sync::ControlMessage;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CentralClientStatus {
    Connecting,
    Connected,
    Offline(String),
    Stopped,
}

pub struct CentralClient {
    shutdown: watch::Sender<bool>,
    status: watch::Receiver<CentralClientStatus>,
    task: JoinHandle<()>,
}

impl CentralClient {
    pub async fn discover_workspace(
        server: &str,
        client_id: &str,
        access_token: Option<&str>,
    ) -> Result<String> {
        let endpoint = sync_endpoint(server)?;
        let request = authenticated_request(endpoint.as_str(), access_token)?;
        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .with_context(|| format!("connect to {endpoint}"))?;
        socket
            .send(Message::Text(
                ControlMessage::client_hello(client_id).encode()?.into(),
            ))
            .await?;
        let hello = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .context("server hello timed out")?
            .context("server disconnected before hello")??;
        let Message::Text(hello) = hello else {
            bail!("server hello was not a text frame");
        };
        let ControlMessage::ServerHello { workspace_id, .. } = ControlMessage::decode(&hello)?
        else {
            bail!("server did not send server_hello");
        };
        socket.close(None).await?;
        Ok(workspace_id)
    }

    pub fn start(
        server: &str,
        client_id: String,
        replica: Arc<CentralReplica>,
        access_token: Option<String>,
    ) -> Result<Self> {
        let endpoint = sync_endpoint(server)?;
        ControlMessage::client_hello(&client_id).validate()?;
        let (shutdown, shutdown_rx) = watch::channel(false);
        let (status_tx, status) = watch::channel(CentralClientStatus::Connecting);
        let task = tokio::spawn(run(
            endpoint,
            client_id,
            replica,
            access_token,
            shutdown_rx,
            status_tx,
        ));
        Ok(Self {
            shutdown,
            status,
            task,
        })
    }

    #[must_use]
    pub fn status(&self) -> CentralClientStatus {
        self.status.borrow().clone()
    }

    #[must_use]
    pub fn subscribe_status(&self) -> watch::Receiver<CentralClientStatus> {
        self.status.clone()
    }

    pub async fn shutdown(self) -> Result<()> {
        let _ = self.shutdown.send(true);
        self.task.await.context("join centralized sync client")?;
        Ok(())
    }
}

async fn run(
    endpoint: Url,
    client_id: String,
    replica: Arc<CentralReplica>,
    access_token: Option<String>,
    mut shutdown: watch::Receiver<bool>,
    status: watch::Sender<CentralClientStatus>,
) {
    let mut delay = Duration::from_millis(250);
    loop {
        if *shutdown.borrow() {
            let _ = status.send(CentralClientStatus::Stopped);
            return;
        }
        let _ = status.send(CentralClientStatus::Connecting);
        match synchronize_once(
            endpoint.as_str(),
            &client_id,
            Arc::clone(&replica),
            access_token.as_deref(),
            &mut shutdown,
            &status,
        )
        .await
        {
            Ok(()) if *shutdown.borrow() => {
                let _ = status.send(CentralClientStatus::Stopped);
                return;
            }
            Ok(()) => {
                let _ = status.send(CentralClientStatus::Offline(
                    "synchronization server disconnected".to_owned(),
                ));
            }
            Err(error) => {
                let _ = status.send(CentralClientStatus::Offline(format!("{error:#}")));
            }
        }
        replica.set_connected_clients(Vec::new()).await;
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    let _ = status.send(CentralClientStatus::Stopped);
                    return;
                }
            }
        }
        delay = (delay * 2).min(Duration::from_secs(15));
    }
}

async fn synchronize_once(
    endpoint: &str,
    client_id: &str,
    replica: Arc<CentralReplica>,
    access_token: Option<&str>,
    shutdown: &mut watch::Receiver<bool>,
    status: &watch::Sender<CentralClientStatus>,
) -> Result<()> {
    let request = authenticated_request(endpoint, access_token)?;
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .with_context(|| format!("connect to {endpoint}"))?;
    let (mut sender, mut receiver) = socket.split();
    sender
        .send(Message::Text(
            ControlMessage::client_hello(client_id).encode()?.into(),
        ))
        .await?;
    let hello = tokio::time::timeout(Duration::from_secs(10), receiver.next())
        .await
        .context("server hello timed out")?
        .context("server disconnected before hello")??;
    let Message::Text(hello) = hello else {
        bail!("server hello was not a text frame");
    };
    let ControlMessage::ServerHello {
        workspace_id,
        clients,
        ..
    } = ControlMessage::decode(&hello)?
    else {
        bail!("server did not send server_hello");
    };
    if workspace_id != replica.workspace_id() {
        bail!(
            "server workspace {workspace_id} does not match local workspace {}",
            replica.workspace_id()
        );
    }
    replica.set_connected_clients(clients).await;
    let _ = status.send(CentralClientStatus::Connected);
    let mut sync_state = SyncState::new();
    let mut events = replica.subscribe();
    if let Some(message) = replica.generate_sync_message(&mut sync_state).await {
        sender.send(Message::Binary(message.into())).await?;
    }

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    sender.send(Message::Close(None)).await?;
                    return Ok(());
                }
            }
            event = events.recv() => {
                match event {
                    Ok(ReplicaEvent::ContentChanged) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Some(message) = replica.generate_sync_message(&mut sync_state).await {
                            sender.send(Message::Binary(message.into())).await?;
                        }
                    }
                    Ok(ReplicaEvent::StatusChanged) => {}
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
            incoming = receiver.next() => {
                match incoming {
                    Some(Ok(Message::Binary(message))) => {
                        replica.receive_sync_message(&mut sync_state, &message).await?;
                        if let Some(message) = replica.generate_sync_message(&mut sync_state).await {
                            sender.send(Message::Binary(message.into())).await?;
                        }
                    }
                    Some(Ok(Message::Text(control))) => match ControlMessage::decode(&control)? {
                        ControlMessage::Presence { clients } => {
                            replica.set_connected_clients(clients).await;
                        }
                        ControlMessage::Error { code, message } => {
                            bail!("server error {code}: {message}");
                        }
                        _ => bail!("unexpected control message after server hello"),
                    },
                    Some(Ok(Message::Ping(bytes))) => sender.send(Message::Pong(bytes)).await?,
                    Some(Ok(Message::Pong(_) | Message::Frame(_))) => {},
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Err(error)) => return Err(error.into()),
                }
            }
        }
    }
}

fn authenticated_request(
    endpoint: &str,
    access_token: Option<&str>,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>> {
    let mut request = endpoint.into_client_request()?;
    if let Some(token) = access_token {
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}"))
                .context("access token is not a valid HTTP header value")?,
        );
    }
    Ok(request)
}

fn sync_endpoint(server: &str) -> Result<Url> {
    let mut endpoint = Url::parse(server).context("server must be an absolute HTTP(S) URL")?;
    match endpoint.scheme() {
        "http" => endpoint.set_scheme("ws").expect("ws is a valid URL scheme"),
        "https" => endpoint
            .set_scheme("wss")
            .expect("wss is a valid URL scheme"),
        "ws" | "wss" => {}
        scheme => bail!("unsupported server URL scheme {scheme:?}"),
    }
    endpoint.set_path("/api/sync");
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls_map_to_the_shared_websocket_endpoint() {
        assert_eq!(
            sync_endpoint("https://notes.example.test/base?q=1")
                .unwrap()
                .as_str(),
            "wss://notes.example.test/api/sync"
        );
        assert_eq!(
            sync_endpoint("http://127.0.0.1:9464").unwrap().as_str(),
            "ws://127.0.0.1:9464/api/sync"
        );
        assert!(sync_endpoint("file:///tmp/xo").is_err());
    }
}
