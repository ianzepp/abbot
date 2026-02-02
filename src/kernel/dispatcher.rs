use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};

use super::error::KernelError;
use super::frame::{Frame, FrameOp};
use super::router::{KernelRouter, Lane};
use super::syscall::{Syscall, SyscallContext};
use super::AuditLog;

fn tap_enabled() -> bool {
    matches!(
        std::env::var("KERNEL_TAP_FRAMES")
            .ok()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "y" | "on"
    )
}

fn tap_filter_all() -> bool {
    matches!(
        std::env::var("KERNEL_TAP_ALL")
            .ok()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "y" | "on"
    )
}

fn tap_should_print(frame: &Frame) -> bool {
    if tap_filter_all() {
        return true;
    }

    // Default: only print high-signal frames.
    matches!(
        frame.op,
        FrameOp::Req | FrameOp::Redirect | FrameOp::Error | FrameOp::Ok | FrameOp::Done
    )
}

fn tap_is_high_signal(frame: &Frame) -> bool {
    matches!(
        frame.op,
        FrameOp::Req | FrameOp::Redirect | FrameOp::Error | FrameOp::Ok | FrameOp::Done
    )
}

fn tap_print(frame: &Frame) {
    // Keep this compact; details are always in logs.db.
    let name = frame.name.as_deref().unwrap_or("");
    let actor = frame.actor.as_deref().unwrap_or("");
    let parent = frame.parent_id.map(|u| u.to_string()).unwrap_or_default();
    let id = frame.id.to_string();
    let op = format!("{:?}", frame.op);

    // Optionally include event kind for readability.
    let event_kind = if frame.op == FrameOp::Event {
        frame
            .data
            .as_ref()
            .and_then(|v| v.get("kind"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
    } else {
        ""
    };

    let high = tap_is_high_signal(frame);
    if !event_kind.is_empty() {
        if high {
            tracing::info!(op, name, actor, id, parent, kind = event_kind, "frame");
        } else {
            tracing::debug!(op, name, actor, id, parent, kind = event_kind, "frame");
        }
    } else {
        if high {
            tracing::info!(op, name, actor, id, parent, "frame");
        } else {
            tracing::debug!(op, name, actor, id, parent, "frame");
        }
    }
}

pub struct KernelReceiver {
    rx: mpsc::Receiver<Frame>,
    queued: Arc<AtomicUsize>,
    low_watermark: usize,
    ping_tx: mpsc::Sender<()>,
}

impl KernelReceiver {
    fn new(
        rx: mpsc::Receiver<Frame>,
        queued: Arc<AtomicUsize>,
        low_watermark: usize,
        ping_tx: mpsc::Sender<()>,
    ) -> Self {
        Self {
            rx,
            queued,
            low_watermark,
            ping_tx,
        }
    }

    pub async fn recv(&mut self) -> Option<Frame> {
        let frame = self.rx.recv().await;
        if frame.is_some() {
            self.ack(1);
        }
        frame
    }

    pub fn ack(&self, processed: usize) {
        if processed == 0 {
            return;
        }

        let mut before = self.queued.load(Ordering::Relaxed);
        loop {
            if before == 0 {
                break;
            }
            let dec = processed.min(before);
            match self.queued.compare_exchange_weak(
                before,
                before - dec,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let after = before - dec;
                    if after <= self.low_watermark {
                        let _ = self.ping_tx.try_send(());
                    }
                    break;
                }
                Err(v) => before = v,
            }
        }
    }
}

pub struct KernelDispatcher {
    handlers: HashMap<String, Arc<dyn Syscall>>,
    tx_capacity: usize,
    low_watermark: usize,
    high_watermark: usize,
    stall_timeout: Duration,
    router: KernelRouter,
    need_lane: Arc<tokio::sync::Mutex<()>>,
    task_lane: Arc<tokio::sync::Mutex<()>>,
    room_lane: Arc<tokio::sync::Mutex<()>>,
    audit: Option<Arc<AuditLog>>,
}

impl KernelDispatcher {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            tx_capacity: 32,
            low_watermark: 8,
            high_watermark: 24,
            stall_timeout: Duration::from_secs(120),
            router: KernelRouter::new(),
            need_lane: Arc::new(tokio::sync::Mutex::new(())),
            task_lane: Arc::new(tokio::sync::Mutex::new(())),
            room_lane: Arc::new(tokio::sync::Mutex::new(())),
            audit: None,
        }
    }

    pub fn set_audit(&mut self, audit: Arc<AuditLog>) {
        self.audit = Some(audit);
    }

    pub fn router_mut(&mut self) -> &mut KernelRouter {
        &mut self.router
    }

    pub fn set_stall_timeout(&mut self, timeout: Duration) {
        if timeout.as_millis() == 0 {
            return;
        }
        self.stall_timeout = timeout;
    }

    pub fn set_backpressure(
        &mut self,
        tx_capacity: usize,
        low_watermark: usize,
        high_watermark: usize,
    ) {
        if tx_capacity == 0 {
            return;
        }
        if low_watermark >= high_watermark {
            return;
        }
        if high_watermark >= tx_capacity {
            return;
        }
        self.tx_capacity = tx_capacity;
        self.low_watermark = low_watermark;
        self.high_watermark = high_watermark;
    }

    pub fn register(&mut self, syscall: Arc<dyn Syscall>) {
        let name = syscall.name().to_string();
        self.handlers.insert(name, syscall);
    }

    pub fn has(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    pub fn list(&self) -> Vec<&str> {
        self.handlers.keys().map(|s| s.as_str()).collect()
    }

    #[instrument(skip(self, req, cancel), fields(call_id = %req.id, name = ?req.name))]
    pub fn dispatch(&self, req: Frame, cwd: PathBuf, cancel: CancellationToken) -> KernelReceiver {
        let (outer_tx, outer_rx) = mpsc::channel(self.tx_capacity);
        let queued = Arc::new(AtomicUsize::new(0));
        let (ping_tx, mut ping_rx) = mpsc::channel::<()>(1);

        if req.op != FrameOp::Req {
            let err = KernelError::invalid_args("dispatch expects a Req frame");
            if outer_tx
                .try_send(Frame::error(req.id, err.to_value()))
                .is_ok()
            {
                queued.fetch_add(1, Ordering::Relaxed);
            }
            return KernelReceiver::new(outer_rx, queued, self.low_watermark, ping_tx);
        }

        let name = match &req.name {
            Some(n) => n.clone(),
            None => {
                let err = KernelError::invalid_args("Req frame missing 'name' field");
                if outer_tx
                    .try_send(Frame::error(req.id, err.to_value()))
                    .is_ok()
                {
                    queued.fetch_add(1, Ordering::Relaxed);
                }
                return KernelReceiver::new(outer_rx, queued, self.low_watermark, ping_tx);
            }
        };

        let handler = match self.handlers.get(&name) {
            Some(h) => Arc::clone(h),
            None => {
                let err = KernelError::not_implemented(&name);
                if outer_tx
                    .try_send(Frame::error(req.id, err.to_value()))
                    .is_ok()
                {
                    queued.fetch_add(1, Ordering::Relaxed);
                }
                return KernelReceiver::new(outer_rx, queued, self.low_watermark, ping_tx);
            }
        };

        let data = req.data.clone().unwrap_or(serde_json::Value::Null);
        let call_id = req.id;
        let actor = req.actor.clone();
        let deadline_ms = req.deadline_ms;

        let lane = self.router.lane_for(&name);
        let need_lane = self.need_lane.clone();
        let task_lane = self.task_lane.clone();
        let room_lane = self.room_lane.clone();

        info!("kernel req received");

        let (inner_tx, mut inner_rx) = mpsc::channel::<Frame>(self.tx_capacity);

        let audit = self.audit.clone();
        let tap = tap_enabled();

        let outer_tx2 = outer_tx.clone();
        let queued2 = queued.clone();
        let low_watermark = self.low_watermark;
        let high_watermark = self.high_watermark;
        let stall_timeout = self.stall_timeout;
        let cancel2 = cancel.clone();
        tokio::spawn(async move {
            let mut paused = false;
            loop {
                if paused {
                    while queued2.load(Ordering::Relaxed) > low_watermark {
                        match tokio::time::timeout(stall_timeout, ping_rx.recv()).await {
                            Ok(Some(_)) => {}
                            Ok(None) => return,
                            Err(_) => {
                                cancel2.cancel();
                                return;
                            }
                        }
                    }
                    paused = false;
                }

                if queued2.load(Ordering::Relaxed) >= high_watermark {
                    paused = true;
                    continue;
                }

                match inner_rx.recv().await {
                    Some(frame) => {
                        if tap && tap_should_print(&frame) {
                            tap_print(&frame);
                        }
                        if let Some(a) = audit.as_ref() {
                            a.append(frame.clone()).await;
                        }
                        if outer_tx2.send(frame).await.is_err() {
                            return;
                        }
                        queued2.fetch_add(1, Ordering::Relaxed);
                    }
                    None => return,
                }
            }
        });

        tokio::spawn(async move {
            let start = Instant::now();

            let ctx = SyscallContext::new(call_id, cwd, cancel.clone())
                .with_actor(actor)
                .with_deadline(deadline_ms);

            let timeout = deadline_ms.map(Duration::from_millis);

            let run = async {
                match lane {
                    Lane::Immediate => handler.execute(&ctx, data, inner_tx.clone()).await,
                    Lane::Need => {
                        let _g = need_lane.lock().await;
                        handler.execute(&ctx, data, inner_tx.clone()).await
                    }
                    Lane::Task => {
                        let _g = task_lane.lock().await;
                        handler.execute(&ctx, data, inner_tx.clone()).await
                    }
                    Lane::Room => {
                        let _g = room_lane.lock().await;
                        handler.execute(&ctx, data, inner_tx.clone()).await
                    }
                }
            };

            let result = if let Some(timeout) = timeout {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        Err(KernelError::cancelled("operation cancelled"))
                    }
                    _ = tokio::time::sleep(timeout) => {
                        Err(KernelError::timeout(format!("syscall exceeded {}ms deadline", timeout.as_millis())))
                    }
                    result = run => result
                }
            } else {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        Err(KernelError::cancelled("operation cancelled"))
                    }
                    result = run => result
                }
            };

            let elapsed = start.elapsed().as_millis();

            match result {
                Ok(()) => {
                    info!(duration_ms = elapsed, "kernel ok emitted");
                }
                Err(e) => {
                    warn!(duration_ms = elapsed, code = %e.code, "kernel error emitted");
                    let _ = inner_tx.send(Frame::error(call_id, e.to_value())).await;
                }
            }
        });

        KernelReceiver::new(outer_rx, queued, self.low_watermark, ping_tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct EchoSyscall;

    #[async_trait::async_trait]
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
            tx.send(Frame::ok(ctx.call_id, data)).await.ok();
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_dispatcher_register_and_dispatch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();

        let mut dispatcher = KernelDispatcher::new();
        dispatcher.register(Arc::new(EchoSyscall));

        assert!(dispatcher.has("test:echo"));
        assert!(!dispatcher.has("test:missing"));

        let req = Frame::req("test:echo", json!({"msg": "hello"}));
        let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

        let response = rx.recv().await.expect("should receive response");
        assert_eq!(response.op, FrameOp::Ok);
        assert_eq!(response.parent_id, Some(req.id));
        assert_eq!(response.data.unwrap()["msg"], "hello");
    }

    #[tokio::test]
    async fn test_dispatcher_unknown_syscall() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();

        let dispatcher = KernelDispatcher::new();

        let req = Frame::req("unknown:syscall", json!({}));
        let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

        let response = rx.recv().await.expect("should receive error");
        assert_eq!(response.op, FrameOp::Error);
        assert_eq!(response.parent_id, Some(req.id));
        assert!(
            response.data.unwrap()["code"]
                .as_str()
                .unwrap()
                .contains("NOT_IMPLEMENTED")
        );
    }

    #[tokio::test]
    async fn test_dispatcher_invalid_frame() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();

        let dispatcher = KernelDispatcher::new();

        let bad_frame = Frame::ok(uuid::Uuid::new_v4(), json!({}));
        let mut rx = dispatcher.dispatch(bad_frame.clone(), workspace, CancellationToken::new());

        let response = rx.recv().await.expect("should receive error");
        assert_eq!(response.op, FrameOp::Error);
    }
}
