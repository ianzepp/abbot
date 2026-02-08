//! Fs:Diff - Generate unified diffs between files with VFS validation
//!
//! Generates diffs between two files. Both paths resolve independently and can
//! be in host mounts, memory, or a mix of both. For host/host diffs, delegates
//! to the system `diff` utility. For any case involving memory, reads both
//! contents and diffs in-process.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hal::{HalProcess, HostHalProcess};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::{MountTable, VfsResolution};

use super::VfsSource;

// =============================================================================
// ARGUMENTS
// =============================================================================

#[derive(Debug, Deserialize)]
struct FsDiffArgs {
    a: String,
    b: String,
    #[serde(default)]
    context_lines: Option<u32>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

pub struct FsDiff {
    vfs: VfsSource,
}

impl FsDiff {
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

impl Default for FsDiff {
    fn default() -> Self {
        Self::new()
    }
}

/// Read content from a VfsResolution (host or memory).
async fn read_content(
    resolution: VfsResolution,
    original_path: &str,
) -> Result<String, KernelError> {
    match resolution {
        VfsResolution::Host(resolved) => tokio::fs::read_to_string(&resolved.host_path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    KernelError::not_found(format!("file not found: {original_path}"))
                } else {
                    KernelError::io(e.to_string())
                }
            }),
        VfsResolution::Memory { path, memory } => memory.read(&path).await,
    }
}

/// Simple unified diff implementation for in-process diffing.
fn unified_diff(a: &str, b: &str, context: usize) -> String {
    let a_lines: Vec<&str> = a.lines().collect();
    let b_lines: Vec<&str> = b.lines().collect();

    // Use a simple LCS-based diff
    let mut output = String::new();

    // Find differing regions
    let mut i = 0;
    let mut j = 0;
    let mut hunks: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut current_hunk: Vec<String> = Vec::new();
    let mut hunk_start_a = 0;
    let mut hunk_start_b = 0;

    while i < a_lines.len() || j < b_lines.len() {
        if i < a_lines.len() && j < b_lines.len() && a_lines[i] == b_lines[j] {
            current_hunk.push(format!(" {}", a_lines[i]));
            i += 1;
            j += 1;
        } else if j < b_lines.len()
            && (i >= a_lines.len() || !a_lines[i..].iter().take(3).any(|l| *l == b_lines[j]))
        {
            if current_hunk.is_empty() {
                hunk_start_a = i + 1;
                hunk_start_b = j + 1;
            }
            current_hunk.push(format!("+{}", b_lines[j]));
            j += 1;
        } else if i < a_lines.len() {
            if current_hunk.is_empty() {
                hunk_start_a = i + 1;
                hunk_start_b = j + 1;
            }
            current_hunk.push(format!("-{}", a_lines[i]));
            i += 1;
        }

        // Flush hunk when we see enough context lines of matching content
        let trailing_context = current_hunk
            .iter()
            .rev()
            .take_while(|l| l.starts_with(' '))
            .count();
        if trailing_context > context && !current_hunk.is_empty() {
            hunks.push((hunk_start_a, hunk_start_b, current_hunk.clone()));
            current_hunk.clear();
        }
    }

    if !current_hunk.is_empty() {
        hunks.push((hunk_start_a, hunk_start_b, current_hunk));
    }

    for (start_a, start_b, lines) in &hunks {
        let a_count = lines
            .iter()
            .filter(|l| l.starts_with('-') || l.starts_with(' '))
            .count();
        let b_count = lines
            .iter()
            .filter(|l| l.starts_with('+') || l.starts_with(' '))
            .count();
        output.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            start_a, a_count, start_b, b_count
        ));
        for line in lines {
            output.push_str(line);
            output.push('\n');
        }
    }

    output
}

#[async_trait]
impl Syscall for FsDiff {
    fn name(&self) -> &'static str {
        "fs:diff"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: FsDiffArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.a.trim().is_empty() || args.b.trim().is_empty() {
            return Err(KernelError::invalid_args(
                "both 'a' and 'b' paths are required",
            ));
        }

        let context = args.context_lines.unwrap_or(3) as usize;

        let res_a = self.vfs.resolve(&args.a)?;
        let res_b = self.vfs.resolve(&args.b)?;

        // If both are host paths, use system diff for best results
        if let (VfsResolution::Host(ra), VfsResolution::Host(rb)) = (&res_a, &res_b) {
            let argv = vec![
                "-U".to_string(),
                context.to_string(),
                ra.host_path.to_string_lossy().to_string(),
                rb.host_path.to_string_lossy().to_string(),
            ];

            let out = HostHalProcess
                .run_bounded(
                    "diff",
                    &argv,
                    &ctx.cwd,
                    None,
                    None,
                    512 * 1024,
                    256 * 1024,
                    Some(ctx.cancel.clone()),
                )
                .await;

            match out {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                    if stdout.trim().is_empty() {
                        if output.code == 0 {
                            let _ = tx.send(Frame::ok(ctx.call_id, json!({"diff": ""}))).await;
                        } else {
                            return Err(KernelError::io(stderr.trim().to_string()));
                        }
                    } else {
                        let clipped: String = stdout.chars().take(20_000).collect();
                        let _ = tx
                            .send(Frame::ok(ctx.call_id, json!({"diff": clipped})))
                            .await;
                    }
                    return Ok(());
                }
                Err(crate::hal::process::HalProcessError::Cancelled { .. }) => {
                    return Err(KernelError::cancelled("diff cancelled"));
                }
                Err(e) => {
                    return Err(KernelError::io(format!("diff error: {e}")));
                }
            }
        }

        // Mixed or memory-only: read both and diff in-process
        let content_a = read_content(res_a, &args.a).await?;
        let content_b = read_content(res_b, &args.b).await?;

        let diff_output = if content_a == content_b {
            String::new()
        } else {
            unified_diff(&content_a, &content_b, context)
        };

        let clipped: String = diff_output.chars().take(20_000).collect();
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"diff": clipped})))
            .await;

        Ok(())
    }
}
