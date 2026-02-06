// Unix domain socket server for raw kernel frame streaming.
//
// WHY: The TUI is a trusted local client. A UDS stream lets us keep the raw
// frame protocol off HTTP/WebSocket entirely (and gate access via filesystem
// permissions).

use std::path::{Path, PathBuf};

use std::os::unix::fs::FileTypeExt;

use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, warn};

use crate::kernel::Frame;
use crate::runtime::Kernel;

#[derive(Serialize)]
#[serde(tag = "type", content = "data")]
enum OutMessage {
    #[serde(rename = "connected")]
    Connected { version: &'static str },

    #[serde(rename = "frame")]
    Frame(Frame),

    #[serde(rename = "error")]
    Error { message: String },
}

fn remove_stale_socket(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let meta = std::fs::symlink_metadata(path)?;
    let ft = meta.file_type();
    if ft.is_socket() {
        std::fs::remove_file(path)?;
        return Ok(());
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!(
            "frames socket path exists and is not a socket: {}",
            path.display()
        ),
    ))
}

fn set_socket_perms_0600(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

async fn write_json_line(stream: &mut UnixStream, msg: &OutMessage) -> std::io::Result<()> {
    let json = serde_json::to_string(msg).unwrap_or_else(|_| {
        serde_json::to_string(&OutMessage::Error {
            message: "failed to serialize message".to_string(),
        })
        .unwrap_or_else(|_| r#"{"type":"error","data":{"message":"serialize failed"}}"#.to_string())
    });
    stream.write_all(json.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    Ok(())
}

async fn handle_client(mut stream: UnixStream) {
    let connected = OutMessage::Connected { version: "0.1.0" };
    if write_json_line(&mut stream, &connected).await.is_err() {
        return;
    }

    let Some(k) = Kernel::get() else {
        let _ = write_json_line(
            &mut stream,
            &OutMessage::Error {
                message: "Kernel not initialized".to_string(),
            },
        )
        .await;
        return;
    };

    // Mirror websocket behavior: send recent frames for initial dataset.
    if let Some(store) = k.frames()
        && let Ok(recent) = store.read_recent(100).await
    {
        for logged in recent {
            if write_json_line(&mut stream, &OutMessage::Frame(logged.frame))
                .await
                .is_err()
            {
                return;
            }
        }
    }

    let mut rx = k.subscribe_frames().await;
    loop {
        match rx.recv().await {
            Ok(frame) => {
                if write_json_line(&mut stream, &OutMessage::Frame(frame))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                debug!(skipped = n, "frames.sock client lagged");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

pub async fn serve_frames_uds(path: PathBuf) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("failed to create socket dir: {e}"))?;
    }

    remove_stale_socket(&path).map_err(|e| e.to_string())?;

    // NOTE: UnixListener::bind will create the filesystem socket node.
    let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
    set_socket_perms_0600(&path);

    debug!(path = %path.display(), "frames UDS listening");

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                tokio::spawn(handle_client(stream));
            }
            Err(e) => {
                warn!(error = %e, "frames UDS accept failed");
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
}
