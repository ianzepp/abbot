use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hal::{HalFs, HostHalFs};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::MountTable;

use super::VfsSource;

#[derive(Debug, Deserialize)]
struct FsMkdirArgs {
    path: String,
    #[serde(default)]
    parents: Option<bool>,
}

pub struct FsMkdir {
    vfs: VfsSource,
}

impl FsMkdir {
    pub fn new() -> Self {
        Self {
            vfs: VfsSource::Global,
        }
    }
}

impl Default for FsMkdir {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for FsMkdir {
    fn name(&self) -> &'static str {
        "fs:mkdir"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        ctx.require_mutation()?;

        let args: FsMkdirArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.path.trim().is_empty() {
            return Err(KernelError::invalid_args("path is required"));
        }

        let vfs = match &self.vfs {
            VfsSource::Disabled => {
                return Err(KernelError::disabled(
                    "filesystem access disabled: no mounts configured",
                ));
            }
            VfsSource::Global => MountTable::global().ok_or_else(|| {
                KernelError::disabled("filesystem access disabled: no mounts configured")
            })?,
            VfsSource::Table(t) => t.as_ref(),
        };

        let resolved = vfs.resolve(&args.path)?;
        let full = &resolved.host_path;
        let ws_root = &resolved.mount.host_path;
        let parents = args.parents.unwrap_or(true);

        let exists = HostHalFs::default().exists(full).await.unwrap_or(false);
        if exists {
            let rel = full
                .strip_prefix(ws_root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| args.path.clone());
            let _ = tx
                .send(Frame::ok(
                    ctx.call_id,
                    json!({"created": false, "path": rel}),
                ))
                .await;
            return Ok(());
        }

        let res = if parents {
            HostHalFs::default().create_dir_all(full).await
        } else {
            HostHalFs::default().create_dir(full).await
        };

        match res {
            Ok(_) => {
                let rel = full
                    .strip_prefix(ws_root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| args.path.clone());
                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({"created": true, "path": rel}),
                    ))
                    .await;
                Ok(())
            }
            Err(e) => Err(KernelError::io(format!("mkdir error: {e}"))),
        }
    }
}
