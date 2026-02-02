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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitOutcome {
    Completed,
    Cancelled,
    TimedOut,
}

fn is_cancelled(cancel: &Option<CancellationToken>) -> bool {
    cancel.as_ref().is_some_and(|c| c.is_cancelled())
}

async fn write_child_stdin(child: &mut tokio::process::Child, stdin: &[u8]) -> Result<(), HalProcessError> {
    use tokio::io::AsyncWriteExt;
    if stdin.is_empty() {
        return Ok(());
    }
    if let Some(mut s) = child.stdin.take() {
        s.write_all(stdin).await.map_err(HalProcessError::io)?;
    }
    Ok(())
}

fn spawn_capture_task<R>(mut reader: Option<R>, max_bytes: usize) -> tokio::task::JoinHandle<Result<(Vec<u8>, bool), HalProcessError>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;

        let mut buf = vec![0u8; 8192];
        let mut captured: Vec<u8> = Vec::new();
        let mut truncated = false;

        let Some(r) = reader.as_mut() else {
            return Ok::<(Vec<u8>, bool), HalProcessError>((captured, false));
        };

        loop {
            let n = r.read(&mut buf).await.map_err(HalProcessError::io)?;
            if n == 0 {
                break;
            }

            let chunk = &buf[..n];
            if captured.len() < max_bytes {
                let remaining = max_bytes - captured.len();
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

        Ok::<(Vec<u8>, bool), HalProcessError>((captured, truncated))
    })
}

async fn wait_child_with_controls(
    child: &mut tokio::process::Child,
    timeout: Option<Duration>,
    cancel: Option<CancellationToken>,
) -> Result<(std::process::ExitStatus, WaitOutcome), HalProcessError> {
    let wait_fut = async { child.wait().await.map_err(HalProcessError::io) };

    match (timeout, cancel) {
        (Some(timeout), Some(cancel)) => {
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = child.kill().await;
                    let status = child.wait().await.map_err(HalProcessError::io)?;
                    Ok((status, WaitOutcome::Cancelled))
                }
                _ = time::sleep(timeout) => {
                    let _ = child.kill().await;
                    let status = child.wait().await.map_err(HalProcessError::io)?;
                    Ok((status, WaitOutcome::TimedOut))
                }
                status = wait_fut => Ok((status?, WaitOutcome::Completed)),
            }
        }
        (Some(timeout), None) => {
            tokio::select! {
                _ = time::sleep(timeout) => {
                    let _ = child.kill().await;
                    let status = child.wait().await.map_err(HalProcessError::io)?;
                    Ok((status, WaitOutcome::TimedOut))
                }
                status = wait_fut => Ok((status?, WaitOutcome::Completed)),
            }
        }
        (None, Some(cancel)) => {
            let status = tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = child.kill().await;
                    child.wait().await.map_err(HalProcessError::io)?
                }
                status = wait_fut => status?,
            };
            let outcome = if cancel.is_cancelled() {
                WaitOutcome::Cancelled
            } else {
                WaitOutcome::Completed
            };
            Ok((status, outcome))
        }
        (None, None) => Ok((wait_fut.await?, WaitOutcome::Completed)),
    }
}

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
        if is_cancelled(&cancel) {
            return Err(HalProcessError::Cancelled {
                program: program.to_string(),
            });
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
        write_child_stdin(&mut child, stdin).await?;

        let out_task = spawn_capture_task(child.stdout.take(), max_stdout_bytes);
        let err_task = spawn_capture_task(child.stderr.take(), max_stderr_bytes);

        let (status, outcome) = wait_child_with_controls(&mut child, timeout, cancel.clone()).await?;

        let (stdout, stdout_truncated) = out_task.await.map_err(HalProcessError::io)??;
        let (stderr, stderr_truncated) = err_task.await.map_err(HalProcessError::io)??;

        let code = status.code().unwrap_or(-1);
        let out = HalCommandOutput {
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
            code,
            success: status.success(),
        };

        if outcome == WaitOutcome::TimedOut {
            let timeout = timeout.unwrap_or_else(|| Duration::from_secs(0));
            return Err(HalProcessError::Timeout {
                program: program.to_string(),
                timeout,
            });
        }

        if outcome == WaitOutcome::Cancelled || is_cancelled(&cancel) {
            return Err(HalProcessError::Cancelled {
                program: program.to_string(),
            });
        }

        Ok(out)
    }
}
