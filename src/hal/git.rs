use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;

use super::process::{HalCommandOutput, HalProcess, HalProcessError, HostHalProcess};

#[async_trait]
pub trait HalGit: Send + Sync {
    async fn run(
        &self,
        cwd: &Path,
        argv: &[String],
        timeout: Option<Duration>,
    ) -> Result<HalCommandOutput, HalProcessError>;
}

#[derive(Debug, Default, Clone)]
pub struct HostHalGit {
    proc: HostHalProcess,
}

#[async_trait]
impl HalGit for HostHalGit {
    async fn run(
        &self,
        cwd: &Path,
        argv: &[String],
        timeout: Option<Duration>,
    ) -> Result<HalCommandOutput, HalProcessError> {
        const MAX_STDOUT_BYTES: usize = 2 * 1024 * 1024;
        const MAX_STDERR_BYTES: usize = 512 * 1024;
        self.proc
            .run_bounded(
                "git",
                argv,
                cwd,
                None,
                timeout,
                MAX_STDOUT_BYTES,
                MAX_STDERR_BYTES,
            )
            .await
    }
}
