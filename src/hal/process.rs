//! Process HAL - Hardware Abstraction Layer for Process Execution
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module provides a trait-based abstraction for executing external processes
//! with support for timeouts, cancellation, bounded output capture, and stdin piping.
//! It wraps tokio::process to provide a safer, more ergonomic API with resource limits.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Bounded output: Enforces limits on stdout/stderr to prevent memory exhaustion
//! - Timeout support: Enforces execution time limits to prevent runaway processes
//! - Cancellation: Supports cooperative cancellation via CancellationToken
//! - Streaming capture: Reads process output asynchronously without blocking
//! - Resource cleanup: Ensures processes are killed and reaped on timeout/cancel
//!
//! TRADE-OFFS
//! ==========
//! - Output truncation: Processes producing output beyond limits are truncated;
//!   callers must check truncated flags and handle partial data appropriately
//! - Kill on timeout: Processes are forcibly killed (SIGKILL) when they exceed
//!   timeout, preventing graceful shutdown. This ensures termination but may
//!   leave external resources in inconsistent states.
//! - Async overhead: All operations are async, adding complexity. This is
//!   necessary for timeout/cancellation integration with tokio runtime.
//!
//! CONCURRENCY
//! ===========
//! Process execution spawns separate tasks for stdout/stderr capture, allowing
//! concurrent reading of both streams. The main task coordinates timeout and
//! cancellation using tokio::select!, ensuring prompt response to control signals.
//!
//! ERROR HANDLING
//! ==============
//! Errors are categorized into I/O failures, timeouts, and cancellations.
//! Timeout and cancellation both kill the process but return different errors
//! to enable distinct handling (retry on timeout, abort on cancellation).

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::time;
use tokio_util::sync::CancellationToken;

// =============================================================================
// TYPES
// =============================================================================
//
// Process execution results capture output, exit status, and truncation flags.

#[derive(Debug, Clone)]
pub struct HalCommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub code: i32,
    pub success: bool,
}

// =============================================================================
// ERRORS
// =============================================================================
//
// Process errors distinguish I/O failures from control flow outcomes (timeout,
// cancellation). This enables callers to implement appropriate retry logic.

#[derive(Debug, Clone)]
pub enum HalProcessError {
    Io(String),
    Cancelled { program: String },
    Timeout { program: String, timeout: Duration },
}

impl HalProcessError {
    /// Create an I/O error from any displayable error.
    ///
    /// WHY: Provides a consistent error constructor for wrapping tokio::io
    /// and std::io errors into a domain-specific type.
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

// =============================================================================
// TRAIT
// =============================================================================
//
// The HalProcess trait defines the process execution abstraction.
//
// WHY multiple methods: Different use cases have different requirements
// (stdin vs no stdin, bounded vs unbounded output). Providing focused methods
// keeps call sites simple while allowing fine-grained control when needed.

#[async_trait]
pub trait HalProcess: Send + Sync {
    /// Execute a process with unbounded output capture.
    ///
    /// WHY: Convenience wrapper for common case where output limits aren't needed.
    async fn run(
        &self,
        program: &str,
        argv: &[String],
        cwd: &Path,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
        cancel: Option<CancellationToken>,
    ) -> Result<HalCommandOutput, HalProcessError>;

    /// Execute a process with stdin and unbounded output capture.
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

    /// Execute a process with bounded output capture.
    ///
    /// WHY bounded: Prevents memory exhaustion when executing untrusted
    /// programs or processing large datasets. Callers can tune limits based
    /// on expected output size.
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

    /// Execute a process with stdin and bounded output capture.
    ///
    /// WHY: Combines stdin piping with output limits, enabling safe execution
    /// of processes that both consume input and produce potentially large output.
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

// =============================================================================
// HOST IMPLEMENTATION
// =============================================================================
//
// HostHalProcess is the production implementation that wraps tokio::process.

#[derive(Debug, Default, Clone)]
pub struct HostHalProcess;

/// Wait outcome distinguishes normal completion from control flow events.
///
/// WHY separate from error: Timeout and cancellation are expected control flow,
/// not exceptional conditions. This type enables clean handling of both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitOutcome {
    Completed,
    Cancelled,
    TimedOut,
}

// =============================================================================
// HELPERS
// =============================================================================
//
// Helper functions encapsulate common patterns and complex logic.

/// Check if a cancellation token is cancelled.
///
/// WHY: Abstracts Option<CancellationToken> handling, providing a clean
/// predicate for early-exit checks.
fn is_cancelled(cancel: &Option<CancellationToken>) -> bool {
    cancel.as_ref().is_some_and(|c| c.is_cancelled())
}

/// Write data to a child process's stdin.
///
/// WHY: Encapsulates stdin piping logic with proper error handling and
/// resource cleanup (stdin is taken from child, preventing double-close).
async fn write_child_stdin(
    child: &mut tokio::process::Child,
    stdin: &[u8],
) -> Result<(), HalProcessError> {
    use tokio::io::AsyncWriteExt;
    if stdin.is_empty() {
        return Ok(());
    }
    if let Some(mut s) = child.stdin.take() {
        s.write_all(stdin).await.map_err(HalProcessError::io)?;
    }
    Ok(())
}

/// Spawn a task to capture process output with size limits.
///
/// WHY separate task: Allows concurrent reading of stdout and stderr without
/// blocking the main task. Each stream is read independently, preventing
/// deadlocks when processes write to both streams.
///
/// WHY return truncated flag: Callers need to know if output was incomplete
/// to handle partial data appropriately (e.g., retry with larger limits,
/// warn the user, fail the operation).
fn spawn_capture_task<R>(
    mut reader: Option<R>,
    max_bytes: usize,
) -> tokio::task::JoinHandle<Result<(Vec<u8>, bool), HalProcessError>>
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
                    // WHY partial chunk: Append as much as fits, then mark truncated
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

/// Wait for a child process with timeout and cancellation support.
///
/// WHY: Encapsulates the complex tokio::select! logic for coordinating timeout,
/// cancellation, and normal completion. Returns both exit status and outcome
/// to enable appropriate error handling.
///
/// TRADE-OFF: Uses SIGKILL on timeout/cancel, preventing graceful shutdown.
/// This ensures termination but may leave external resources inconsistent.
/// Alternative would be SIGTERM with grace period, but that adds complexity
/// and doesn't guarantee termination.
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

// =============================================================================
// TRAIT IMPLEMENTATION
// =============================================================================

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
        self.run_bounded(
            program,
            argv,
            cwd,
            env,
            timeout,
            usize::MAX,
            usize::MAX,
            cancel,
        )
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
        // -------------------------------------------------------------------------
        // PHASE 1: PRE-EXECUTION VALIDATION
        // Check for early cancellation before spawning the process
        // -------------------------------------------------------------------------
        if is_cancelled(&cancel) {
            return Err(HalProcessError::Cancelled {
                program: program.to_string(),
            });
        }

        // -------------------------------------------------------------------------
        // PHASE 2: PROCESS SETUP
        // Configure command with arguments, environment, and piped I/O
        // -------------------------------------------------------------------------
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

        // -------------------------------------------------------------------------
        // PHASE 3: SPAWN AND STDIN HANDLING
        // Spawn the process and write stdin data if provided
        // -------------------------------------------------------------------------
        let mut child = cmd.spawn().map_err(HalProcessError::io)?;
        write_child_stdin(&mut child, stdin).await?;

        // -------------------------------------------------------------------------
        // PHASE 4: OUTPUT CAPTURE SETUP
        // Spawn concurrent tasks to capture stdout and stderr
        // -------------------------------------------------------------------------
        let out_task = spawn_capture_task(child.stdout.take(), max_stdout_bytes);
        let err_task = spawn_capture_task(child.stderr.take(), max_stderr_bytes);

        // -------------------------------------------------------------------------
        // PHASE 5: WAIT WITH CONTROLS
        // Wait for process completion, timeout, or cancellation
        // -------------------------------------------------------------------------
        let (status, outcome) =
            wait_child_with_controls(&mut child, timeout, cancel.clone()).await?;

        // -------------------------------------------------------------------------
        // PHASE 6: OUTPUT COLLECTION
        // Await capture tasks and collect output data
        // -------------------------------------------------------------------------
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

        // -------------------------------------------------------------------------
        // PHASE 7: OUTCOME HANDLING
        // Convert wait outcome into appropriate error or success result
        // -------------------------------------------------------------------------
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
