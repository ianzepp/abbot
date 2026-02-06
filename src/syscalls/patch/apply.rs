use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hal::{HalProcess, HostHalProcess};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::MountTable;

#[derive(Debug, Deserialize)]
struct PatchApplyArgs {
    patch: String,
}

pub struct PatchApply;

impl PatchApply {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for PatchApply {
    fn name(&self) -> &'static str {
        "patch:apply"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        ctx.require_mutation()?;

        let args: PatchApplyArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let diff = args.patch.trim();
        if diff.is_empty() {
            return Err(KernelError::invalid_args("patch is empty"));
        }

        if !diff.contains("---") || !diff.contains("+++") {
            return Err(KernelError::invalid_args(
                "invalid diff format (missing --- or +++ headers)",
            ));
        }

        // Validate all paths in diff headers are within VFS mounts
        let vfs = MountTable::global().ok_or_else(|| {
            KernelError::disabled("filesystem access disabled: no mounts configured")
        })?;

        for line in diff.lines() {
            let path = if let Some(rest) = line.strip_prefix("+++ ") {
                Some(rest)
            } else if let Some(rest) = line.strip_prefix("--- ") {
                Some(rest)
            } else {
                None
            };

            if let Some(raw_path) = path {
                if raw_path == "/dev/null" || raw_path.starts_with("/dev/null") {
                    continue;
                }
                let stripped = raw_path
                    .strip_prefix("a/")
                    .or_else(|| raw_path.strip_prefix("b/"))
                    .unwrap_or(raw_path)
                    .split('\t')
                    .next()
                    .unwrap_or(raw_path);
                if stripped.is_empty() {
                    continue;
                }
                vfs.resolve(stripped).map_err(|_| {
                    KernelError::forbidden(format!(
                        "patch references path outside workspace: {}",
                        stripped
                    ))
                })?;
            }
        }

        let argv = vec![
            "-p1".to_string(),
            "--no-backup-if-mismatch".to_string(),
            "-r-".to_string(),
        ];

        let output = HostHalProcess::default()
            .run_with_stdin_bytes_bounded(
                "patch",
                &argv,
                &ctx.cwd,
                None,
                None,
                diff.as_bytes(),
                256 * 1024,
                256 * 1024,
                None,
            )
            .await;

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if output.success {
                    let clipped: String = stdout.trim_end().chars().take(4000).collect();
                    let _ = tx
                        .send(Frame::ok(ctx.call_id, json!({"stdout": clipped})))
                        .await;
                    Ok(())
                } else {
                    let msg = if !stderr.trim().is_empty() {
                        stderr.trim().to_string()
                    } else {
                        stdout.trim().to_string()
                    };
                    Err(KernelError::io(format!("patch failed: {msg}")))
                }
            }
            Err(crate::hal::process::HalProcessError::Cancelled { .. }) => {
                Err(KernelError::cancelled("patch cancelled"))
            }
            Err(e) => Err(KernelError::io(format!("patch error: {e}"))),
        }
    }
}
