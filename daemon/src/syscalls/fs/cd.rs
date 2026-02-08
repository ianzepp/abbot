//! Fs:Cd - Change VFS working directory
//!
//! Resolves a path against the current VFS CWD, validates that it exists and
//! is a directory, and returns the resolved absolute path. The caller (agent
//! loop) is responsible for updating its mutable `vfs_cwd` state.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::{MountTable, VfsResolution, resolve_vfs_path};

use super::VfsSource;

// =============================================================================
// ARGUMENTS
// =============================================================================

#[derive(Debug, Deserialize)]
struct FsCdArgs {
    path: String,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

pub struct FsCd {
    vfs: VfsSource,
}

impl FsCd {
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

impl Default for FsCd {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for FsCd {
    fn name(&self) -> &'static str {
        "fs:cd"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: FsCdArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.path.trim().is_empty() {
            return Err(KernelError::invalid_args("path is required"));
        }

        // Resolve the path against current VFS CWD
        let resolved_path = resolve_vfs_path(&args.path, &ctx.vfs_cwd)?;

        // Validate that the path exists and is a directory via VFS resolution
        let resolution = self.vfs.resolve(&resolved_path)?;

        match resolution {
            VfsResolution::Host(ref r) => {
                if !r.host_path.exists() {
                    return Err(KernelError::not_found(format!(
                        "directory not found: {}",
                        resolved_path
                    )));
                }
                if !r.host_path.is_dir() {
                    return Err(KernelError::invalid_args(format!(
                        "not a directory: {}",
                        resolved_path
                    )));
                }
            }
            VfsResolution::Memory {
                ref path,
                ref memory,
            } => {
                if !memory.is_dir(path).await {
                    return Err(KernelError::not_found(format!(
                        "directory not found: {}",
                        resolved_path
                    )));
                }
            }
        }

        tx.send(Frame::ok(ctx.call_id, json!({ "cwd": resolved_path })))
            .await
            .ok();

        Ok(())
    }
}
