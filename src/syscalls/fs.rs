use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hal::{HalFs, HostHalFs};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::{MountMode, MountTable};

#[derive(Clone)]
enum VfsSource {
    Global,
    Disabled,
    Table(Arc<MountTable>),
}

#[derive(Debug, Deserialize)]
struct FsReadArgs {
    path: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

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

    /// Constructor for testing with a custom VFS mount table.
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

        ctx.check_cancelled()?;

        let content = self
            .fs
            .read_to_string(&resolved.host_path)
            .await
            .map_err(|e| {
                if e.to_string().contains("No such file") || e.to_string().contains("not found") {
                    KernelError::not_found(format!("file not found: {}", args.path))
                } else {
                    KernelError::io(e.to_string())
                }
            })?;

        let result = if args.offset.is_some() || args.limit.is_some() {
            let lines: Vec<&str> = content.lines().collect();
            let offset = args.offset.unwrap_or(0);
            let limit = args.limit.unwrap_or(lines.len());

            let selected: Vec<&str> = lines.into_iter().skip(offset).take(limit).collect();
            selected.join("\n")
        } else {
            content
        };

        tx.send(Frame::ok(ctx.call_id, json!({ "content": result })))
            .await
            .ok();

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct FsWriteArgs {
    path: String,
    content: String,
    #[serde(default)]
    create_dirs: bool,
}

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

    /// Constructor for testing with a custom VFS mount table.
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

        if resolved.mount.mode == MountMode::Ro {
            return Err(KernelError::readonly(format!(
                "mount '{}' is read-only",
                resolved.mount.prefix
            )));
        }

        ctx.require_mutation()?;

        if let Some(parent) = resolved.host_path.parent() {
            if !parent.exists() {
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

    fn make_ctx_with_actor(cwd: &std::path::Path, actor: &str) -> SyscallContext {
        SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
            .with_actor(Some(actor.to_string()))
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
        let syscall = FsWrite::new();
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
        assert!(err.code == "E_DISABLED" || err.code == "E_FORBIDDEN");
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
}
