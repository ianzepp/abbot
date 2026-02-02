use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

use abbot::kernel::{Frame, FrameOp, KernelDispatcher, KernelError, Syscall, SyscallContext};

struct EchoSyscall;

#[async_trait]
impl Syscall for EchoSyscall {
    fn name(&self) -> &'static str {
        "test:echo"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let _ = tx.send(Frame::ok(ctx.call_id, data)).await;
        Ok(())
    }
}

struct ProgressSyscall {
    count: usize,
}

#[async_trait]
impl Syscall for ProgressSyscall {
    fn name(&self) -> &'static str {
        "test:progress"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        for i in 0..self.count {
            ctx.check_cancelled()?;
            let _ = tx.send(Frame::progress(ctx.call_id, json!({"i": i}))).await;
        }
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"progress": self.count})))
            .await;
        Ok(())
    }
}

struct SlowSyscall {
    sleep: Duration,
}

#[async_trait]
impl Syscall for SlowSyscall {
    fn name(&self) -> &'static str {
        "test:slow"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        tokio::time::sleep(self.sleep).await;
        ctx.check_cancelled()?;
        let _ = tx.send(Frame::ok(ctx.call_id, json!({"slept_ms": self.sleep.as_millis()}))).await;
        Ok(())
    }
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok().and_then(|v| v.parse::<usize>().ok())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_kernel_dispatcher_throughput_parallel_echo() {
    let iters = env_usize("ABBOT_KERNEL_DISPATCH_ITERS").unwrap_or(10_000);
    let concurrency = env_usize("ABBOT_KERNEL_DISPATCH_CONCURRENCY").unwrap_or(256);

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    let mut dispatcher = KernelDispatcher::new();
    dispatcher.register(Arc::new(EchoSyscall));
    let dispatcher = Arc::new(dispatcher);

    let sem = Arc::new(Semaphore::new(concurrency));
    let start = Instant::now();

    let whole_run = async {
        let mut join_set = tokio::task::JoinSet::new();
        for i in 0..iters {
            let permit = sem.clone().acquire_owned().await.unwrap();
            let dispatcher = dispatcher.clone();
            let workspace = workspace.clone();
            join_set.spawn(async move {
                let _permit = permit;
                let req = Frame::req("test:echo", json!({"i": i}));
                let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());
                let response = rx.recv().await.ok_or("missing response")?;
                if response.op != FrameOp::Ok {
                    return Err("unexpected non-ok response") as Result<(), &'static str>;
                }
                if response.parent_id != Some(req.id) {
                    return Err("wrong parent_id") as Result<(), &'static str>;
                }
                let data = response.data.ok_or("missing data")?;
                if data["i"].as_u64() != Some(i as u64) {
                    return Err("wrong payload") as Result<(), &'static str>;
                }
                Ok(())
            });
        }

        while let Some(res) = join_set.join_next().await {
            res.expect("task panicked")?;
        }
        Ok::<(), &'static str>(())
    };

    tokio::time::timeout(Duration::from_secs(20), whole_run)
        .await
        .expect("dispatch stress test timed out")
        .expect("dispatch stress test failed");

    let elapsed = start.elapsed();
    let ops_per_sec = (iters as f64) / elapsed.as_secs_f64().max(0.000_001);
    eprintln!(
        "kernel dispatch throughput: iters={} concurrency={} elapsed={:?} ops/s={:.0}",
        iters, concurrency, elapsed, ops_per_sec
    );

    assert!(
        elapsed <= Duration::from_secs(20),
        "unexpectedly slow dispatch: {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_kernel_dispatcher_backpressure_progress_stream() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    let mut dispatcher = KernelDispatcher::new();
    dispatcher.register(Arc::new(ProgressSyscall { count: 64 }));

    let req = Frame::req("test:progress", json!({}));
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let mut progress = 0usize;
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for frame")
            .expect("channel closed unexpectedly");

        match frame.op {
            FrameOp::Progress => {
                assert_eq!(frame.parent_id, Some(req.id));
                progress += 1;
            }
            FrameOp::Ok => {
                assert_eq!(frame.parent_id, Some(req.id));
                break;
            }
            other => panic!("unexpected frame op: {other:?}"),
        }
    }

    assert_eq!(progress, 64);
}

#[tokio::test]
async fn test_kernel_dispatcher_enforces_deadline() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    let mut dispatcher = KernelDispatcher::new();
    dispatcher.register(Arc::new(SlowSyscall {
        sleep: Duration::from_millis(200),
    }));

    let req = Frame::req("test:slow", json!({})).with_deadline(50);
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timed out waiting for first frame")
        .expect("channel closed unexpectedly");

    assert_eq!(first.op, FrameOp::Error);
    assert_eq!(first.parent_id, Some(req.id));
    assert_eq!(first.data.as_ref().unwrap()["code"], "E_TIMEOUT");

    let no_more = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
    assert!(
        no_more.is_err() || no_more.unwrap().is_none(),
        "unexpected extra frame after timeout"
    );
}
