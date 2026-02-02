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
        self.proc.run("git", argv, cwd, None, timeout).await
    }
}
