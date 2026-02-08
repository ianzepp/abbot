//! Fs:Read - Read file content with VFS path validation
//!
//! Reads files from either host mounts or the in-memory filesystem,
//! with optional line slicing (offset/limit) for partial file access.

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
struct FsReadArgs {
    path: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

pub struct FsRead {
    fs: Arc<dyn HalFs>,
    vfs: VfsSource,
}

impl FsRead {
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

impl Default for FsRead {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply line slicing (offset/limit) to content.
fn slice_lines(content: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    if offset.is_some() || limit.is_some() {
        let lines: Vec<&str> = content.lines().collect();
        let offset = offset.unwrap_or(0);
        let limit = limit.unwrap_or(lines.len());
        let selected: Vec<&str> = lines.into_iter().skip(offset).take(limit).collect();
        selected.join("\n")
    } else {
        content.to_string()
    }
}

#[async_trait]
impl Syscall for FsRead {
    fn name(&self) -> &'static str {
        "fs:read"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: FsReadArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.path.is_empty() {
            return Err(KernelError::invalid_args(
                "'path' is required and cannot be empty",
            ));
        }

        let resolution = self.vfs.resolve(&args.path)?;

        ctx.check_cancelled()?;

        let result = match resolution {
            VfsResolution::Host(resolved) => {
                let content = self
                    .fs
                    .read_to_string(&resolved.host_path)
                    .await
                    .map_err(|e| {
                        if e.to_string().contains("No such file")
                            || e.to_string().contains("not found")
                        {
                            KernelError::not_found(format!("file not found: {}", args.path))
                        } else {
                            KernelError::io(e.to_string())
                        }
                    })?;
                slice_lines(&content, args.offset, args.limit)
            }
            VfsResolution::Memory { path, memory } => {
                let content = memory.read(&path).await?;
                slice_lines(&content, args.offset, args.limit)
            }
        };

        tx.send(Frame::ok(ctx.call_id, json!({ "content": result })))
            .await
            .ok();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    fn make_ctx(cwd: &std::path::Path) -> SyscallContext {
        SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
    }

    #[tokio::test]
    async fn test_fs_read_no_vfs_returns_disabled() {
        let syscall = FsRead::disabled();
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "path": "/test.txt" }), tx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_DISABLED");
    }

    #[tokio::test]
    async fn test_fs_read_empty_path_rejected() {
        let syscall = FsRead::new();
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall.execute(&ctx, json!({ "path": "" }), tx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARGS");
    }

    #[tokio::test]
    async fn test_fs_read_memory_roundtrip() {
        let table = MountTable::from_config(vec![]).unwrap();
        // Write to memory first
        table
            .memory()
            .write("/test.txt", b"hello memory")
            .await
            .unwrap();

        let syscall = FsRead {
            fs: Arc::new(HostHalFs),
            vfs: VfsSource::Table(Arc::new(table)),
        };
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "path": "/test.txt" }), tx)
            .await;
        assert!(result.is_ok());

        let frame = rx.recv().await.unwrap();
        let data = frame.data.unwrap();
        assert_eq!(data["content"].as_str().unwrap(), "hello memory");
    }
}
