use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tokio::process::Command;
use tokio::time;

#[derive(Debug, Clone)]
pub struct HalCommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub code: i32,
    pub success: bool,
}

#[derive(Debug, Clone)]
pub enum HalProcessError {
    Io(String),
    Cancelled { program: String },
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
            HalProcessError::Cancelled { program } => write!(f, "{program} cancelled"),
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
        cancel: Option<CancellationToken>,
    ) -> Result<HalCommandOutput, HalProcessError>;

    async fn run_with_stdin_bytes(
        &self,
        program: &str,
        argv: &[String],
        cwd: &Path,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
        stdin: &[u8],
        cancel: Option<CancellationToken>,
    ) -> Result<HalCommandOutput, HalProcessError>;

    async fn run_bounded(
        &self,
        program: &str,
        argv: &[String],
        cwd: &Path,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
        max_stdout_bytes: usize,
        max_stderr_bytes: usize,
        cancel: Option<CancellationToken>,
    ) -> Result<HalCommandOutput, HalProcessError>;

    async fn run_with_stdin_bytes_bounded(
        &self,
        program: &str,
        argv: &[String],
        cwd: &Path,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
        stdin: &[u8],
        max_stdout_bytes: usize,
        max_stderr_bytes: usize,
        cancel: Option<CancellationToken>,
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
        cancel: Option<CancellationToken>,
    ) -> Result<HalCommandOutput, HalProcessError> {
        self.run_bounded(program, argv, cwd, env, timeout, usize::MAX, usize::MAX, cancel)
            .await
    }

    async fn run_with_stdin_bytes(
        &self,
        program: &str,
        argv: &[String],
        cwd: &Path,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
        stdin: &[u8],
        cancel: Option<CancellationToken>,
    ) -> Result<HalCommandOutput, HalProcessError> {
        self.run_with_stdin_bytes_bounded(
            program,
            argv,
            cwd,
            env,
            timeout,
            stdin,
            usize::MAX,
            usize::MAX,
            cancel,
        )
        .await
    }

    async fn run_bounded(
        &self,
        program: &str,
        argv: &[String],
        cwd: &Path,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
        max_stdout_bytes: usize,
        max_stderr_bytes: usize,
        cancel: Option<CancellationToken>,
    ) -> Result<HalCommandOutput, HalProcessError> {
        self.run_with_stdin_bytes_bounded(
            program,
            argv,
            cwd,
            env,
            timeout,
            &[],
            max_stdout_bytes,
            max_stderr_bytes,
            cancel,
        )
        .await
    }

    async fn run_with_stdin_bytes_bounded(
        &self,
        program: &str,
        argv: &[String],
        cwd: &Path,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
        stdin: &[u8],
        max_stdout_bytes: usize,
        max_stderr_bytes: usize,
        cancel: Option<CancellationToken>,
    ) -> Result<HalCommandOutput, HalProcessError> {
        use tokio::io::AsyncReadExt;
        use tokio::io::AsyncWriteExt;

        if let Some(cancel) = &cancel {
            if cancel.is_cancelled() {
                return Err(HalProcessError::Cancelled {
                    program: program.to_string(),
                });
            }
        }

        let mut cmd = Command::new(program);
        cmd.args(argv)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(env) = env {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }

        let mut child = cmd.spawn().map_err(HalProcessError::io)?;

        if !stdin.is_empty() {
            if let Some(mut s) = child.stdin.take() {
                s.write_all(stdin).await.map_err(HalProcessError::io)?;
            }
        }

        let mut out_reader = child.stdout.take();
        let mut err_reader = child.stderr.take();

        let out_task = tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let mut captured: Vec<u8> = Vec::new();
            let mut truncated = false;
            if let Some(ref mut r) = out_reader {
                loop {
                    let n = r.read(&mut buf).await.map_err(HalProcessError::io)?;
                    if n == 0 {
                        break;
                    }
                    let chunk = &buf[..n];
                    if captured.len() < max_stdout_bytes {
                        let remaining = max_stdout_bytes - captured.len();
                        if chunk.len() <= remaining {
                            captured.extend_from_slice(chunk);
                        } else {
                            captured.extend_from_slice(&chunk[..remaining]);
                            truncated = true;
                        }
                    } else {
                        truncated = true;
                    }
                }
            }
            Ok::<(Vec<u8>, bool), HalProcessError>((captured, truncated))
        });

        let err_task = tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let mut captured: Vec<u8> = Vec::new();
            let mut truncated = false;
            if let Some(ref mut r) = err_reader {
                loop {
                    let n = r.read(&mut buf).await.map_err(HalProcessError::io)?;
                    if n == 0 {
                        break;
                    }
                    let chunk = &buf[..n];
                    if captured.len() < max_stderr_bytes {
                        let remaining = max_stderr_bytes - captured.len();
                        if chunk.len() <= remaining {
                            captured.extend_from_slice(chunk);
                        } else {
                            captured.extend_from_slice(&chunk[..remaining]);
                            truncated = true;
                        }
                    } else {
                        truncated = true;
                    }
                }
            }
            Ok::<(Vec<u8>, bool), HalProcessError>((captured, truncated))
        });

        let wait_fut = async { child.wait().await.map_err(HalProcessError::io) };
        let mut timed_out: Option<Duration> = None;
        let mut cancelled = false;
        let status = match (timeout, cancel) {
            (Some(timeout), Some(cancel)) => {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        cancelled = true;
                        let _ = child.kill().await;
                        child.wait().await.map_err(HalProcessError::io)?
                    }
                    res = time::timeout(timeout, wait_fut) => {
                        match res {
                            Ok(status) => status?,
                            Err(_) => {
                                timed_out = Some(timeout);
                                let _ = child.kill().await;
                                child.wait().await.map_err(HalProcessError::io)?
                            }
                        }
                    }
                }
            }
            (Some(timeout), None) => {
                match time::timeout(timeout, wait_fut).await {
                    Ok(res) => res?,
                    Err(_) => {
                        timed_out = Some(timeout);
                        let _ = child.kill().await;
                        child.wait().await.map_err(HalProcessError::io)?
                    }
                }
            }
            (None, Some(cancel)) => {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        cancelled = true;
                        let _ = child.kill().await;
                        child.wait().await.map_err(HalProcessError::io)?
                    }
                    res = wait_fut => res?,
                }
            }
            (None, None) => wait_fut.await?,
        };

        let (stdout, stdout_truncated) = out_task
            .await
            .map_err(HalProcessError::io)??;
        let (stderr, stderr_truncated) = err_task
            .await
            .map_err(HalProcessError::io)??;

        let code = status.code().unwrap_or(-1);
        let out = HalCommandOutput {
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
            code,
            success: status.success(),
        };

        if let Some(timeout) = timed_out {
            return Err(HalProcessError::Timeout {
                program: program.to_string(),
                timeout,
            });
        }

        if cancelled {
            return Err(HalProcessError::Cancelled {
                program: program.to_string(),
            });
        }

        Ok(out)
    }
}
