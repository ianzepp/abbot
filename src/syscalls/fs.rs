use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::hal::{HalFs, HostHalFs};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

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
}

impl FsRead {
    pub fn new() -> Self {
        Self {
            fs: Arc::new(HostHalFs),
        }
    }

    pub fn with_fs(fs: Arc<dyn HalFs>) -> Self {
        Self { fs }
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
            return Err(KernelError::invalid_args("'path' is required and cannot be empty"));
        }

        let validated_path = ctx.validate_path(&args.path)?;

        ctx.check_cancelled()?;

        let content = self.fs.read_to_string(&validated_path).await.map_err(|e| {
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
}

impl FsWrite {
    pub fn new() -> Self {
        Self {
            fs: Arc::new(HostHalFs),
        }
    }

    pub fn with_fs(fs: Arc<dyn HalFs>) -> Self {
        Self { fs }
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
            return Err(KernelError::invalid_args("'path' is required and cannot be empty"));
        }

        let target = if args.path.starts_with('/') {
            std::path::PathBuf::from(&args.path)
        } else {
            ctx.cwd.join(&args.path)
        };

        let workspace_canonical = ctx.workspace_root.canonicalize().map_err(|e| {
            KernelError::internal(format!("workspace root cannot be canonicalized: {e}"))
        })?;

        if let Some(parent) = target.parent() {
            let parent_canonical = if parent.exists() {
                parent.canonicalize().map_err(|e| KernelError::io(e.to_string()))?
            } else if args.create_dirs {
                self.fs.create_dir_all(parent).await.map_err(|e| KernelError::io(e.to_string()))?;
                parent.canonicalize().map_err(|e| KernelError::io(e.to_string()))?
            } else {
                return Err(KernelError::not_found(format!(
                    "parent directory does not exist: {}",
                    parent.display()
                )));
            };

            if !parent_canonical.starts_with(&workspace_canonical) {
                return Err(KernelError::forbidden(format!(
                    "path escapes workspace: {} is outside {}",
                    target.display(),
                    ctx.workspace_root.display()
                )));
            }
        }

        ctx.check_cancelled()?;

        self.fs
            .write(&target, args.content.as_bytes())
            .await
            .map_err(|e| KernelError::io(e.to_string()))?;

        tx.send(Frame::ok(
            ctx.call_id,
            json!({
                "path": target.display().to_string(),
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
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    async fn make_ctx(workspace: &std::path::Path) -> SyscallContext {
        SyscallContext::new(
            Uuid::new_v4(),
            workspace.to_path_buf(),
            workspace.to_path_buf(),
            CancellationToken::new(),
        )
    }

    #[tokio::test]
    async fn test_fs_read_success() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, "line1\nline2\nline3").unwrap();

        let syscall = FsRead::new();
        let ctx = make_ctx(tmp.path()).await;
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "path": "test.txt" }), tx)
            .await;

        assert!(result.is_ok());
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.op, crate::kernel::FrameOp::Ok);
        assert!(frame.data.unwrap()["content"].as_str().unwrap().contains("line1"));
    }

    #[tokio::test]
    async fn test_fs_read_with_limit() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, "line1\nline2\nline3\nline4\nline5").unwrap();

        let syscall = FsRead::new();
        let ctx = make_ctx(tmp.path()).await;
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "path": "test.txt", "offset": 1, "limit": 2 }), tx)
            .await;

        assert!(result.is_ok());
        let frame = rx.recv().await.unwrap();
        let content = frame.data.unwrap()["content"].as_str().unwrap().to_string();
        assert!(content.contains("line2"));
        assert!(content.contains("line3"));
        assert!(!content.contains("line1"));
        assert!(!content.contains("line4"));
    }

    #[tokio::test]
    async fn test_fs_read_not_found() {
        let tmp = TempDir::new().unwrap();
        let syscall = FsRead::new();
        let ctx = make_ctx(tmp.path()).await;
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "path": "nonexistent.txt" }), tx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_fs_write_success() {
        let tmp = TempDir::new().unwrap();
        let syscall = FsWrite::new();
        let ctx = make_ctx(tmp.path()).await;
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(
                &ctx,
                json!({ "path": "output.txt", "content": "hello world" }),
                tx,
            )
            .await;

        assert!(result.is_ok());
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.op, crate::kernel::FrameOp::Ok);

        let written = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
        assert_eq!(written, "hello world");
    }

    #[tokio::test]
    async fn test_fs_write_workspace_escape() {
        let tmp = TempDir::new().unwrap();
        let syscall = FsWrite::new();
        let ctx = make_ctx(tmp.path()).await;
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(
                &ctx,
                json!({ "path": "/etc/hacked.txt", "content": "bad" }),
                tx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }
}
