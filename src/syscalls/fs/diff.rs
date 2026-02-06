use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hal::{HalProcess, HostHalProcess};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::MountTable;

use super::VfsSource;

#[derive(Debug, Deserialize)]
struct FsDiffArgs {
    a: String,
    b: String,
    #[serde(default)]
    context_lines: Option<u32>,
}

pub struct FsDiff {
    vfs: VfsSource,
}

impl FsDiff {
    pub fn new() -> Self {
        Self {
            vfs: VfsSource::Global,
        }
    }
}

impl Default for FsDiff {
    fn default() -> Self {
        Self::new()
    }
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
            return Err(KernelError::invalid_args("both 'a' and 'b' paths are required"));
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

        let resolved_a = vfs.resolve(&args.a)?;
        let resolved_b = vfs.resolve(&args.b)?;

        let context = args.context_lines.unwrap_or(3);
        let argv = vec![
            "-U".to_string(),
            context.to_string(),
            resolved_a.host_path.to_string_lossy().to_string(),
            resolved_b.host_path.to_string_lossy().to_string(),
        ];

        let out = HostHalProcess::default()
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
                        let _ = tx
                            .send(Frame::ok(ctx.call_id, json!({"diff": ""})))
                            .await;
                    } else {
                        return Err(KernelError::io(stderr.trim().to_string()));
                    }
                } else {
                    // Clip to 20k chars
                    let clipped: String = stdout.chars().take(20_000).collect();
                    let _ = tx
                        .send(Frame::ok(ctx.call_id, json!({"diff": clipped})))
                        .await;
                }
                Ok(())
            }
            Err(crate::hal::process::HalProcessError::Cancelled { .. }) => {
                Err(KernelError::cancelled("diff cancelled"))
            }
            Err(e) => Err(KernelError::io(format!("diff error: {e}"))),
        }
    }
}
