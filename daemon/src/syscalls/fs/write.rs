//! Fs:Write - Write file content with VFS and mount mode validation
//!
//! Writes files to either host mounts (with read-only enforcement) or the
//! in-memory filesystem. Requires mutation permission.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hal::{HalFs, HostHalFs};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::{MountMode, MountTable, VfsResolution};

use super::VfsSource;

// =============================================================================
// ARGUMENTS
// =============================================================================

#[derive(Debug, Deserialize)]
struct FsWriteArgs {
    path: String,
    content: String,
    #[serde(default)]
    create_dirs: bool,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

pub struct FsWrite {
    fs: Arc<dyn HalFs>,
    vfs: VfsSource,
}

impl FsWrite {
    pub fn new() -> Self {
        Self {
            fs: Arc::new(HostHalFs),
            vfs: VfsSource::Global,
        }
    }

    pub fn disabled() -> Self {
        Self {
            fs: Arc::new(HostHalFs),
            vfs: VfsSource::Disabled,
        }
    }

    pub fn with_fs(fs: Arc<dyn HalFs>) -> Self {
        Self {
            fs,
            vfs: VfsSource::Global,
        }
    }

    #[allow(dead_code)]
    pub fn with_vfs(fs: Arc<dyn HalFs>, vfs: Arc<MountTable>) -> Self {
        Self {
            fs,
            vfs: VfsSource::Table(vfs),
        }
    }
}

impl Default for FsWrite {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for FsWrite {
    fn name(&self) -> &'static str {
        "fs:write"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: FsWriteArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.path.is_empty() {
            return Err(KernelError::invalid_args(
                "'path' is required and cannot be empty",
            ));
        }

        let resolution = self.vfs.resolve_with_cwd(&args.path, &ctx.vfs_cwd)?;

        match resolution {
            VfsResolution::Host(resolved) => {
                // Mount mode enforcement
                if resolved.mount.mode == MountMode::Ro {
                    return Err(KernelError::readonly(format!(
                        "mount '{}' is read-only",
                        resolved.mount.prefix
                    )));
                }

                // Actor authorization
                ctx.require_mutation()?;

                // Parent directory creation (optional)
                if let Some(parent) = resolved.host_path.parent()
                    && !parent.exists()
                {
                    if args.create_dirs {
                        self.fs
                            .create_dir_all(parent)
                            .await
                            .map_err(|e| KernelError::io(e.to_string()))?;
                    } else {
                        return Err(KernelError::not_found(format!(
                            "parent directory does not exist: {}",
                            parent.display()
                        )));
                    }
                }

                ctx.check_cancelled()?;

                self.fs
                    .write(&resolved.host_path, args.content.as_bytes())
                    .await
                    .map_err(|e| KernelError::io(e.to_string()))?;

                tx.send(Frame::ok(
                    ctx.call_id,
                    json!({
                        "path": resolved.host_path.display().to_string(),
                        "bytes_written": args.content.len()
                    }),
                ))
                .await
                .ok();
            }
            VfsResolution::Memory { path, memory } => {
                // Memory is always read-write, but still require mutation permission
                ctx.require_mutation()?;

                memory.write(&path, args.content.as_bytes()).await?;

                tx.send(Frame::ok(
                    ctx.call_id,
                    json!({
                        "path": path,
                        "bytes_written": args.content.len()
                    }),
                ))
                .await
                .ok();
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    fn make_ctx_with_actor(cwd: &std::path::Path, actor: &str) -> SyscallContext {
        SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
            .with_actor(Some(actor.to_string()))
    }

    #[tokio::test]
    async fn test_fs_write_no_vfs_returns_disabled() {
        let syscall = FsWrite::disabled();
        let ctx = make_ctx_with_actor(std::path::Path::new("/tmp"), "head/test");
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(
                &ctx,
                json!({ "path": "/output.txt", "content": "hello world" }),
                tx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_DISABLED");
    }

    #[tokio::test]
    async fn test_fs_write_hand_scope_rejected() {
        let table =
            MountTable::from_config(vec![], PathBuf::from("/tmp/vfs-test-sandbox")).unwrap();
        let syscall = FsWrite {
            fs: Arc::new(HostHalFs),
            vfs: VfsSource::Table(Arc::new(table)),
        };
        let ctx = make_ctx_with_actor(std::path::Path::new("/tmp"), "hand/test");
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(
                &ctx,
                json!({ "path": "/output.txt", "content": "hello" }),
                tx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.code == "E_FORBIDDEN");
    }

    #[tokio::test]
    async fn test_fs_write_memory() {
        let table =
            MountTable::from_config(vec![], PathBuf::from("/tmp/vfs-test-sandbox")).unwrap();
        let syscall = FsWrite {
            fs: Arc::new(HostHalFs),
            vfs: VfsSource::Table(Arc::new(table.clone())),
        };
        let ctx = make_ctx_with_actor(std::path::Path::new("/tmp"), "head/test");
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(
                &ctx,
                json!({ "path": "/tmp/scratch.txt", "content": "memory write" }),
                tx,
            )
            .await;
        assert!(result.is_ok());

        let frame = rx.recv().await.unwrap();
        let data = frame.data.unwrap();
        assert_eq!(data["path"].as_str().unwrap(), "/tmp/scratch.txt");
        assert_eq!(data["bytes_written"].as_u64().unwrap(), 12);

        // Verify content in memory
        let content = table.memory().read("/tmp/scratch.txt").await.unwrap();
        assert_eq!(content, "memory write");
    }
}
