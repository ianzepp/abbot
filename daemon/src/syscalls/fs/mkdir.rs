//! Fs:Mkdir - Create directories with VFS path validation
//!
//! Creates directories in either host mounts or the in-memory filesystem.
//! Requires mutation permission.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hal::{HalFs, HostHalFs};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::{MountTable, VfsResolution};

use super::VfsSource;

// =============================================================================
// ARGUMENTS
// =============================================================================

#[derive(Debug, Deserialize)]
struct FsMkdirArgs {
    path: String,
    #[serde(default)]
    parents: Option<bool>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

pub struct FsMkdir {
    vfs: VfsSource,
}

impl FsMkdir {
    pub fn new() -> Self {
        Self {
            vfs: VfsSource::Global,
        }
    }

    #[allow(dead_code)]
    pub fn with_vfs(vfs: Arc<MountTable>) -> Self {
        Self {
            vfs: VfsSource::Table(vfs),
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

        let parents = args.parents.unwrap_or(true);
        let resolution = self.vfs.resolve(&args.path)?;

        match resolution {
            VfsResolution::Host(resolved) => {
                let full = &resolved.host_path;
                let ws_root = &resolved.mount.host_path;

                // Idempotent: success if directory already exists
                let exists = HostHalFs.exists(full).await.unwrap_or(false);
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
                    HostHalFs.create_dir_all(full).await
                } else {
                    HostHalFs.create_dir(full).await
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
            VfsResolution::Memory { path, memory } => {
                // Idempotent: success if directory already exists
                if memory.is_dir(&path).await {
                    let _ = tx
                        .send(Frame::ok(
                            ctx.call_id,
                            json!({"created": false, "path": path}),
                        ))
                        .await;
                    return Ok(());
                }

                memory.mkdir(&path, parents).await?;

                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({"created": true, "path": path}),
                    ))
                    .await;
                Ok(())
            }
        }
    }
}
