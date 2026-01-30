use std::path::Path;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

use crate::bus::{Message, Origin};
use crate::runtime::RuntimeBus;

use super::WireMessage;

pub struct SocketListener {
    bus: RuntimeBus,
    path: String,
}

impl SocketListener {
    pub fn new(bus: RuntimeBus, path: impl Into<String>) -> Self {
        Self {
            bus,
            path: path.into(),
        }
    }

    pub async fn start(self: Arc<Self>) -> std::io::Result<()> {
        let path = Path::new(&self.path);

        if path.exists() {
            std::fs::remove_file(path)?;
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(path)?;
        tracing::info!(path = %self.path, "socket listener started");

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let bus = self.bus.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(bus, stream).await {
                            tracing::debug!(error = %e, "client disconnected");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                }
            }
        }
    }
}

async fn handle_connection(bus: RuntimeBus, stream: UnixStream) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let mut rx = bus.hub().read().await.subscribe_all();

    tracing::debug!("client connected");

    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line? {
                    Some(line) => {
                        if let Err(e) = handle_incoming(&bus, &line).await {
                            tracing::warn!(error = %e, "failed to handle incoming message");
                        }
                    }
                    None => break,
                }
            }
            msg = rx.recv() => {
                match msg {
                    Ok(msg) => {
                        let wire = WireMessage::from(&msg);
                        let json = serde_json::to_string(&wire).unwrap_or_default();
                        if writer.write_all(json.as_bytes()).await.is_err() {
                            break;
                        }
                        if writer.write_all(b"\n").await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(dropped = n, "client lagged, dropped messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    tracing::debug!("client disconnected");
    Ok(())
}

async fn handle_incoming(bus: &RuntimeBus, line: &str) -> Result<(), String> {
    let wire: WireMessage = serde_json::from_str(line).map_err(|e| format!("parse error: {}", e))?;
    let mut msg: Message = wire.try_into()?;

    // Force origin to Human for socket clients
    msg.origin = Origin::Human;

    bus.publish(msg).await;
    Ok(())
}
