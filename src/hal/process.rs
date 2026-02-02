use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::time;

#[derive(Debug, Clone)]
pub struct HalCommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: i32,
    pub success: bool,
}

#[derive(Debug, Clone)]
pub enum HalProcessError {
    Io(String),
    Timeout {
        program: String,
        timeout: Duration,
    },
}

impl HalProcessError {
    pub fn io(e: impl std::fmt::Display) -> Self {
        Self::Io(e.to_string())
    }
}

impl std::fmt::Display for HalProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HalProcessError::Io(msg) => write!(f, "{msg}"),
            HalProcessError::Timeout { program, timeout } => {
                write!(f, "{program} timed out after {:?}", timeout)
            }
        }
    }
}

impl std::error::Error for HalProcessError {}

#[async_trait]
pub trait HalProcess: Send + Sync {
    async fn run(
        &self,
        program: &str,
        argv: &[String],
        cwd: &Path,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
    ) -> Result<HalCommandOutput, HalProcessError>;

    async fn run_with_stdin_bytes(
        &self,
        program: &str,
        argv: &[String],
        cwd: &Path,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
        stdin: &[u8],
    ) -> Result<HalCommandOutput, HalProcessError>;
}

#[derive(Debug, Default, Clone)]
pub struct HostHalProcess;

#[async_trait]
impl HalProcess for HostHalProcess {
    async fn run(
        &self,
        program: &str,
        argv: &[String],
        cwd: &Path,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
    ) -> Result<HalCommandOutput, HalProcessError> {
        let mut cmd = Command::new(program);
        cmd.args(argv).current_dir(cwd);

        if let Some(env) = env {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }

        let fut = cmd.output();

        let output = if let Some(timeout) = timeout {
            match time::timeout(timeout, fut).await {
                Ok(res) => res.map_err(HalProcessError::io)?,
                Err(_) => {
                    return Err(HalProcessError::Timeout {
                        program: program.to_string(),
                        timeout,
                    });
                }
            }
        } else {
            fut.await.map_err(HalProcessError::io)?
        };

        let status = output.status;
        let code = status.code().unwrap_or(-1);

        Ok(HalCommandOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            code,
            success: status.success(),
        })
    }

    async fn run_with_stdin_bytes(
        &self,
        program: &str,
        argv: &[String],
        cwd: &Path,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
        stdin: &[u8],
    ) -> Result<HalCommandOutput, HalProcessError> {
        use tokio::io::AsyncWriteExt;

        let mut cmd = Command::new(program);
        cmd.args(argv)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped());

        if let Some(env) = env {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }

        let mut child = cmd.spawn().map_err(HalProcessError::io)?;
        let fut = async {
            if let Some(mut s) = child.stdin.take() {
                s.write_all(stdin).await.map_err(HalProcessError::io)?;
            }
            child.wait_with_output().await.map_err(HalProcessError::io)
        };

        let output = if let Some(timeout) = timeout {
            match time::timeout(timeout, fut).await {
                Ok(res) => res?,
                Err(_) => {
                    return Err(HalProcessError::Timeout {
                        program: program.to_string(),
                        timeout,
                    });
                }
            }
        } else {
            fut.await?
        };

        let status = output.status;
        let code = status.code().unwrap_or(-1);

        Ok(HalCommandOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            code,
            success: status.success(),
        })
    }
}
