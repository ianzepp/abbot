//! Fs:List - List directory contents with VFS path validation and glob filtering
//!
//! Lists directories from either host mounts or the in-memory filesystem.
//! At the root ("/"), merges memory entries with host mount prefixes.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::{MountTable, VfsResolution};

use super::VfsSource;

// =============================================================================
// ARGUMENTS
// =============================================================================

#[derive(Debug, Deserialize)]
struct FsListArgs {
    #[serde(default)]
    path: String,
    #[serde(default)]
    pattern: String,
    #[serde(default)]
    recursive: bool,
    #[serde(default)]
    max_results: Option<usize>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

pub struct FsList {
    vfs: VfsSource,
}

impl FsList {
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

impl Default for FsList {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for FsList {
    fn name(&self) -> &'static str {
        "fs:list"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: FsListArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let max_results = args.max_results.unwrap_or(1000).min(5000);

        let path = if args.path.trim().is_empty() {
            ".".to_string()
        } else {
            args.path.clone()
        };

        let matcher: Option<GlobSet> = if !args.pattern.trim().is_empty() {
            let glob = Glob::new(args.pattern.trim())
                .map_err(|e| KernelError::invalid_args(format!("invalid pattern: {e}")))?;
            let mut builder = GlobSetBuilder::new();
            builder.add(glob);
            builder.build().ok()
        } else {
            None
        };

        let resolution = self.vfs.resolve(&path)?;

        match resolution {
            VfsResolution::Host(resolved) => {
                let base = &resolved.host_path;

                if !base.exists() {
                    return Err(KernelError::not_found(format!(
                        "directory not found: {}",
                        args.path
                    )));
                }

                let ws_root = &resolved.mount.host_path;
                let mut out = Vec::new();
                let depth = if args.recursive { usize::MAX } else { 1 };

                for entry in walkdir::WalkDir::new(base)
                    .follow_links(false)
                    .max_depth(depth)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    if entry.path() == base.as_path() {
                        continue;
                    }

                    let rel = entry
                        .path()
                        .strip_prefix(ws_root)
                        .ok()
                        .map(|p| p.to_string_lossy().to_string());
                    let Some(rel) = rel else { continue };

                    if let Some(m) = &matcher {
                        let name = entry.file_name().to_string_lossy();
                        if !m.is_match(name.as_ref()) {
                            continue;
                        }
                    }

                    out.push(rel);
                    if out.len() >= max_results {
                        break;
                    }
                }

                out.sort();
                let truncated = out.len() >= max_results;

                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({"matches": out, "truncated": truncated}),
                    ))
                    .await;
            }
            VfsResolution::Memory { path, memory } => {
                let mut entries = BTreeSet::new();

                // List memory entries under this path
                let mem_children = memory.list(&path).await?;
                for child in mem_children {
                    // Return child name relative to the listed path
                    let name = child
                        .strip_prefix(&path)
                        .unwrap_or(&child)
                        .trim_start_matches('/');

                    if let Some(m) = &matcher
                        && !m.is_match(name)
                    {
                        continue;
                    }
                    entries.insert(name.to_string());
                }

                // If listing root, also include mount prefixes as virtual directories
                if path == "/" {
                    let table = self.vfs.table()?;
                    for prefix in table.mount_prefixes() {
                        // Top-level component of mount prefix
                        let name = prefix
                            .strip_prefix('/')
                            .unwrap_or(&prefix)
                            .split('/')
                            .next()
                            .unwrap_or(&prefix);

                        if let Some(m) = &matcher
                            && !m.is_match(name)
                        {
                            continue;
                        }
                        entries.insert(name.to_string());
                    }
                }

                let mut out: Vec<String> = entries.into_iter().take(max_results).collect();
                out.sort();
                let truncated = out.len() >= max_results;

                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({"matches": out, "truncated": truncated}),
                    ))
                    .await;
            }
        }

        Ok(())
    }
}
