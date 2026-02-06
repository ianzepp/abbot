use async_trait::async_trait;
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::MountTable;

use super::VfsSource;

#[derive(Debug, Deserialize)]
struct FsSearchArgs {
    query: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    include: String,
    #[serde(default)]
    regex: bool,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default)]
    max_results: Option<usize>,
}

pub struct FsSearch {
    vfs: VfsSource,
}

impl FsSearch {
    pub fn new() -> Self {
        Self {
            vfs: VfsSource::Global,
        }
    }
}

impl Default for FsSearch {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for FsSearch {
    fn name(&self) -> &'static str {
        "fs:search"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: FsSearchArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let query = args.query.clone();
        if query.trim().is_empty() {
            return Err(KernelError::invalid_args("query is required"));
        }

        let max_results = args.max_results.unwrap_or(200).min(5000);

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

        let path = if args.path.trim().is_empty() {
            ".".to_string()
        } else {
            args.path.clone()
        };

        let resolved = vfs.resolve(&path)?;
        let base = &resolved.host_path;

        if !base.exists() {
            return Err(KernelError::not_found(format!(
                "directory not found: {}",
                args.path
            )));
        }

        let include_set: Option<GlobSet> = if !args.include.trim().is_empty() {
            let glob = Glob::new(args.include.trim())
                .map_err(|e| KernelError::invalid_args(format!("invalid include: {e}")))?;
            let mut builder = GlobSetBuilder::new();
            builder.add(glob);
            Some(
                builder
                    .build()
                    .map_err(|e| KernelError::invalid_args(format!("invalid include: {e}")))?,
            )
        } else {
            None
        };

        let re = if args.regex {
            let pat = if args.case_sensitive {
                query.clone()
            } else {
                format!("(?i){}", query)
            };
            Some(
                Regex::new(&pat)
                    .map_err(|e| KernelError::invalid_args(format!("invalid regex: {e}")))?,
            )
        } else {
            None
        };

        let ws_root = &resolved.mount.host_path;
        let mut matches = Vec::new();

        for entry in walkdir::WalkDir::new(base)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy();
            if let Some(inc) = &include_set {
                if !inc.is_match(file_name.as_ref()) {
                    continue;
                }
            }

            let path = entry.path();
            let content = match tokio::fs::read_to_string(path).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            for (i, line) in content.lines().enumerate() {
                let hit = if let Some(re) = &re {
                    re.is_match(line)
                } else if args.case_sensitive {
                    line.contains(&query)
                } else {
                    line.to_ascii_lowercase()
                        .contains(&query.to_ascii_lowercase())
                };

                if hit {
                    let rel = path
                        .strip_prefix(ws_root)
                        .ok()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.to_string_lossy().to_string());

                    // Clip long lines to 400 chars
                    let text: String = line.chars().take(400).collect();
                    matches.push(json!({
                        "path": rel,
                        "line": i + 1,
                        "text": text
                    }));
                    if matches.len() >= max_results {
                        break;
                    }
                }
            }
            if matches.len() >= max_results {
                break;
            }
        }

        let truncated = matches.len() >= max_results;
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"matches": matches, "truncated": truncated}),
            ))
            .await;

        Ok(())
    }
}
