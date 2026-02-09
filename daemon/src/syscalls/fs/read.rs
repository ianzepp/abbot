//! Fs:Read - Read file content with VFS path validation
//!
//! Reads files from either host mounts or the in-memory filesystem,
//! with optional line slicing (offset/limit) for partial file access.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

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
    /// Maximum bytes to read from host/memory (default: 512KiB).
    /// Values are clamped to a hard ceiling to prevent memory abuse.
    #[serde(default)]
    max_bytes: Option<usize>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

pub struct FsRead {
    vfs: VfsSource,
}

impl FsRead {
    pub fn new() -> Self {
        Self {
            vfs: VfsSource::Global,
        }
    }

    pub fn disabled() -> Self {
        Self {
            vfs: VfsSource::Disabled,
        }
    }

    #[allow(dead_code)]
    pub fn with_vfs(vfs: Arc<MountTable>) -> Self {
        Self {
            vfs: VfsSource::Table(vfs),
        }
    }
}

impl Default for FsRead {
    fn default() -> Self {
        Self::new()
    }
}

const DEFAULT_MAX_BYTES: usize = 512 * 1024;
const HARD_MAX_BYTES: usize = 4 * 1024 * 1024;

fn clamp_max_bytes(v: Option<usize>) -> usize {
    v.unwrap_or(DEFAULT_MAX_BYTES).clamp(1, HARD_MAX_BYTES)
}

/// Apply line slicing (offset/limit) to content without materializing all lines.
fn slice_lines(content: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    if offset.is_none() && limit.is_none() {
        return content.to_string();
    }

    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(usize::MAX);

    let mut out = String::new();
    let mut idx = 0usize;
    let mut taken = 0usize;
    for line in content.lines() {
        if idx < offset {
            idx += 1;
            continue;
        }
        if taken >= limit {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        taken += 1;
        idx += 1;
    }
    out
}

fn truncate_string_to_bytes(s: &str, max_bytes: usize) -> (String, bool) {
    if s.len() <= max_bytes {
        return (s.to_string(), false);
    }
    let end = s.floor_char_boundary(max_bytes);
    (s[..end].to_string(), true)
}

fn decode_utf8_prefix(bytes: Vec<u8>) -> Result<String, KernelError> {
    match std::str::from_utf8(&bytes) {
        Ok(s) => Ok(s.to_string()),
        Err(e) => {
            // If this is just a truncated codepoint at the end, drop bytes until valid.
            if e.error_len().is_none() {
                let valid = e.valid_up_to();
                Ok(std::str::from_utf8(&bytes[..valid]).unwrap().to_string())
            } else {
                Err(KernelError::io("file is not valid UTF-8"))
            }
        }
    }
}

async fn read_host_utf8_limited(
    host_path: &std::path::Path,
    vfs_path: &str,
    max_bytes: usize,
) -> Result<(String, bool, usize), KernelError> {
    let mut f = match tokio::fs::File::open(host_path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(KernelError::not_found(format!(
                "file not found: {vfs_path}"
            )));
        }
        Err(e) => return Err(KernelError::io(e.to_string())),
    };

    let mut buf = Vec::with_capacity(max_bytes.min(64 * 1024) + 1);
    let mut limited = (&mut f).take((max_bytes + 1) as u64);
    limited
        .read_to_end(&mut buf)
        .await
        .map_err(|e| KernelError::io(e.to_string()))?;

    let truncated = buf.len() > max_bytes;
    if truncated {
        buf.truncate(max_bytes);
    }
    let bytes_read = buf.len();
    let s = decode_utf8_prefix(buf)?;
    Ok((s, truncated, bytes_read))
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

        let max_bytes = clamp_max_bytes(args.max_bytes);

        let resolution = self.vfs.resolve_with_cwd(&args.path, &ctx.vfs_cwd)?;

        ctx.check_cancelled()?;

        let (content, truncated, bytes_read) = match resolution {
            VfsResolution::Host(resolved) => {
                let (content, truncated, bytes_read) =
                    read_host_utf8_limited(&resolved.host_path, &args.path, max_bytes).await?;
                (
                    slice_lines(&content, args.offset, args.limit),
                    truncated,
                    bytes_read,
                )
            }
            VfsResolution::Memory { path, memory } => {
                let content = memory.read(&path).await?;
                let (content, truncated) = truncate_string_to_bytes(&content, max_bytes);
                let bytes_read = content.len();
                (
                    slice_lines(&content, args.offset, args.limit),
                    truncated,
                    bytes_read,
                )
            }
        };

        tx.send(Frame::ok(
            ctx.call_id,
            json!({
                "content": content,
                "truncated": truncated,
                "bytes_read": bytes_read,
                "max_bytes": max_bytes,
            }),
        ))
        .await
        .ok();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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
        let table =
            MountTable::from_config(vec![], PathBuf::from("/tmp/vfs-test-sandbox")).unwrap();
        // Write to memory first (use /tmp path for memory-backed access)
        table
            .memory()
            .write("/tmp/test.txt", b"hello memory")
            .await
            .unwrap();

        let syscall = FsRead {
            vfs: VfsSource::Table(Arc::new(table)),
        };
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "path": "/tmp/test.txt" }), tx)
            .await;
        assert!(result.is_ok());

        let frame = rx.recv().await.unwrap();
        let data = frame.data.unwrap();
        assert_eq!(data["content"].as_str().unwrap(), "hello memory");
    }
}
