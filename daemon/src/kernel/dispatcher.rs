//! Kernel Dispatcher - Syscall routing and execution runtime
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The dispatcher routes syscall requests to registered handlers, enforces
//! concurrency lanes to prevent deadlocks, and manages backpressure for
//! streaming response frames.
//!
//! Flow:
//! - dispatch() validates the request frame and looks up the handler
//! - Spawns two tasks: exec (syscall execution) and pump (response streaming)
//! - Exec task acquires lane lock (if needed), invokes syscall, emits frames
//! - Pump task streams frames to caller, applying backpressure when queued > high watermark
//!
//! Backpressure prevents unbounded memory growth when a syscall emits frames
//! faster than the caller consumes them (e.g., streaming LLM responses).
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Lane-based concurrency: Serialize mutations, parallelize reads/long-polls
//! - Backpressure via watermarks: Pause emission when queue is full, resume when drained
//! - Stall timeout: Cancel syscall if backpressure persists (prevents deadlock)
//! - Audit integration: All frames (request + response) are logged before delivery
//!
//! TRADE-OFFS
//! ==========
//! - Backpressure adds complexity but prevents OOM on slow clients
//! - Stall timeout may cancel legitimate slow operations; tune per workload
//! - Full audit logging impacts throughput; disable for high-frequency syscalls if needed

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::FrameStore;
use super::error::KernelError;
use super::frame::{Frame, FrameOp};
use super::router::{KernelRouter, Lane};
use super::syscall::{Syscall, SyscallContext};

// =============================================================================
// DEBUGGING SUPPORT
// =============================================================================

// WHY tap functions exist: Kernel frame flow debugging without requiring
// separate tracing filters. Controlled via KERNEL_TAP_FRAMES env var.

static TAP_SEQ: AtomicU64 = AtomicU64::new(0);

fn tap_enabled() -> bool {
    !matches!(
        std::env::var("KERNEL_TAP_FRAMES")
            .ok()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "no" | "off"
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

    // WHY default filter: Req/Ok/Done/Error are high-signal; Item/Bytes are
    // high-frequency (streaming text) and clutter logs.
    matches!(
        frame.op,
        FrameOp::Req | FrameOp::Error | FrameOp::Ok | FrameOp::Done
    )
}

fn tap_is_high_signal(frame: &Frame) -> bool {
    matches!(
        frame.op,
        FrameOp::Req | FrameOp::Error | FrameOp::Ok | FrameOp::Done
    )
}

/// Extract room from trace or data.
fn frame_room(frame: &Frame) -> Option<&str> {
    frame
        .trace
        .as_ref()
        .and_then(|t| t.get("room"))
        .and_then(|s| s.as_str())
        .or_else(|| {
            frame
                .data
                .as_ref()
                .and_then(|d| d.get("room"))
                .and_then(|s| s.as_str())
        })
}

fn tap_print(frame: &Frame, ctx_name: Option<&str>, ctx_actor: Option<&str>) {
    let seq = TAP_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let op = format!("{:?}", frame.op).to_ascii_lowercase();
    let name = frame.name.as_deref().or(ctx_name).unwrap_or("");
    let actor = frame.actor.as_deref().or(ctx_actor).unwrap_or("");
    let room = frame_room(frame).unwrap_or("");

    if tap_is_high_signal(frame) {
        tracing::info!(target: "tap", seq, op = %op, name, actor, room);
    } else {
        tracing::debug!(target: "tap", seq, op = %op, name, actor, room);
    }
}

// =============================================================================
// KERNEL RECEIVER
// =============================================================================

/// Receiver for syscall response frames with backpressure acknowledgment.
///
/// WHY this exists: Callers must signal when frames are consumed (via ack or
/// recv) to allow the pump task to resume emission when queue drains below
/// low watermark.
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

    /// Receive next frame and automatically acknowledge consumption.
    ///
    /// WHY auto-ack: Simplifies caller code; most callers process one frame
    /// at a time and want immediate backpressure release.
    pub async fn recv(&mut self) -> Option<Frame> {
        let frame = self.rx.recv().await;
        if frame.is_some() {
            self.ack(1);
        }
        frame
    }

    /// Acknowledge processing of frames to release backpressure.
    ///
    /// WHY explicit ack: Callers that batch-process frames can defer ack until
    /// batch completes, avoiding unnecessary wake-ups.
    ///
    /// WHY ping when below watermark: Notify pump task that queue has space;
    /// pump resumes emission if it was paused.
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
                        // WHY try_send: Ping channel is size 1; if already
                        // full, pump task is already notified.
                        let _ = self.ping_tx.try_send(());
                    }
                    break;
                }
                Err(v) => before = v,
            }
        }
    }
}

// =============================================================================
// KERNEL DISPATCHER
// =============================================================================

/// Syscall dispatcher with lane-based concurrency and backpressure control.
///
/// WHY this exists: Centralizes syscall registration, routing, and execution
/// with configurable backpressure to prevent memory exhaustion from slow
/// consumers.
pub struct KernelDispatcher {
    handlers: HashMap<String, Arc<dyn Syscall>>,
    tx_capacity: usize,
    low_watermark: usize,
    high_watermark: usize,
    stall_timeout: Duration,
    router: KernelRouter,
    need_lane: Arc<tokio::sync::Mutex<()>>,
    room_lane: Arc<tokio::sync::Mutex<()>>,
    frames: Option<Arc<FrameStore>>,
    broadcast_tx: broadcast::Sender<Frame>,
}

impl Default for KernelDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl KernelDispatcher {
    pub fn new() -> Self {
        let (broadcast_tx, _) = broadcast::channel(256);
        Self {
            handlers: HashMap::new(),
            tx_capacity: 32,
            low_watermark: 8,
            high_watermark: 24,
            stall_timeout: Duration::from_secs(120),
            router: KernelRouter::new(),
            need_lane: Arc::new(tokio::sync::Mutex::new(())),
            room_lane: Arc::new(tokio::sync::Mutex::new(())),
            frames: None,
            broadcast_tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Frame> {
        self.broadcast_tx.subscribe()
    }

    pub fn broadcast_sender(&self) -> broadcast::Sender<Frame> {
        self.broadcast_tx.clone()
    }

    /// Attach frame store for frame persistence.
    ///
    /// WHY: All frames (request + response) are persisted for debugging,
    /// replay, and compliance.
    pub fn set_frames(&mut self, frames: Arc<FrameStore>) {
        self.frames = Some(frames);
    }

    pub fn router_mut(&mut self) -> &mut KernelRouter {
        &mut self.router
    }

    /// Configure stall timeout for backpressure.
    ///
    /// WHY: If backpressure persists beyond timeout, syscall is cancelled to
    /// prevent indefinite blocking. Tune based on expected consumer latency.
    pub fn set_stall_timeout(&mut self, timeout: Duration) {
        if timeout.as_millis() == 0 {
            return;
        }
        self.stall_timeout = timeout;
    }

    /// Configure backpressure watermarks.
    ///
    /// WHY watermarks: Pause emission at high watermark, resume at low watermark.
    /// Prevents oscillation (pause/resume thrashing) by using hysteresis.
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

    /// Register a syscall handler.
    ///
    /// WHY: Syscalls are registered at kernel initialization. Name must be
    /// <namespace>:<verb> per refactor spec.
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

    /// Dispatch a syscall request and return a receiver for response frames.
    ///
    /// WHY spawn exec + pump tasks: Exec runs syscall (may block on lane lock),
    /// pump streams responses with backpressure. Separating allows backpressure
    /// to operate independently of syscall execution.
    ///
    /// CONCURRENCY: Exec task acquires lane lock if needed; pump task streams
    /// frames to caller. If caller is slow, pump pauses at high watermark and
    /// resumes at low watermark (or times out and cancels exec).
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

        let mut data = req.data.clone().unwrap_or(serde_json::Value::Null);
        // Extract _vfs_cwd from data payload (injected by dispatch_tool) for SyscallContext
        let vfs_cwd = data
            .as_object_mut()
            .and_then(|obj| obj.remove("_vfs_cwd"))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "/".to_string());
        let call_id = req.id;
        let actor = req.actor.clone();
        let deadline_ms = req.deadline_ms;
        let req_for_audit = req.clone();

        let lane = self.router.lane_for(&name);
        let need_lane = self.need_lane.clone();
        let room_lane = self.room_lane.clone();

        let (inner_tx, mut inner_rx) = mpsc::channel::<Frame>(self.tx_capacity);

        let frames_for_pump = self.frames.clone();
        let frames_for_exec = self.frames.clone();
        let broadcast_tx = self.broadcast_tx.clone();
        let broadcast_tx_exec = self.broadcast_tx.clone();
        let tap = tap_enabled();

        let outer_tx2 = outer_tx.clone();
        let queued2 = queued.clone();
        let low_watermark = self.low_watermark;
        let high_watermark = self.high_watermark;
        let stall_timeout = self.stall_timeout;
        let cancel2 = cancel.clone();
        let pump_name = name.clone();
        let pump_actor = actor.clone();
        tokio::spawn(async move {
            let mut paused = false;
            loop {
                // -------------------------------------------------------------------------
                // BACKPRESSURE PAUSE: Wait for queue to drain below low watermark
                // WHY: Prevents unbounded memory growth when syscall emits faster than
                // caller consumes. Stall timeout ensures we don't wait forever.
                // -------------------------------------------------------------------------
                if paused {
                    while queued2.load(Ordering::Relaxed) > low_watermark {
                        match tokio::time::timeout(stall_timeout, ping_rx.recv()).await {
                            Ok(Some(_)) => {}
                            Ok(None) => return,
                            Err(_) => {
                                // WHY cancel on stall: Syscall is emitting frames
                                // but caller isn't consuming; prevent indefinite block
                                cancel2.cancel();
                                return;
                            }
                        }
                    }
                    paused = false;
                }

                // -------------------------------------------------------------------------
                // BACKPRESSURE CHECK: Pause if queue exceeds high watermark
                // -------------------------------------------------------------------------
                if queued2.load(Ordering::Relaxed) >= high_watermark {
                    paused = true;
                    continue;
                }

                // -------------------------------------------------------------------------
                // FRAME STREAMING: Deliver response frames to caller
                // WHY audit + broadcast: Frames are logged and broadcast to monitors
                // before delivery to ensure observability even if caller drops frames
                // -------------------------------------------------------------------------
                match inner_rx.recv().await {
                    Some(frame) => {
                        if tap && tap_should_print(&frame) {
                            tap_print(&frame, Some(&pump_name), pump_actor.as_deref());
                        }
                        if let Some(a) = frames_for_pump.as_ref() {
                            a.append(frame.clone()).await;
                        }
                        let _ = broadcast_tx.send(frame.clone());
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
            // -------------------------------------------------------------------------
            // AUDIT REQUEST: Persist request before execution
            // WHY: Ensures audit ordering matches request -> response even if execution
            // fails or syscall crashes. Broadcast for monitoring/debugging.
            // -------------------------------------------------------------------------
            if tap && tap_should_print(&req_for_audit) {
                tap_print(&req_for_audit, None, None);
            }
            if let Some(a) = frames_for_exec.as_ref() {
                a.append(req_for_audit.clone()).await;
            }
            let _ = broadcast_tx_exec.send(req_for_audit);

            // -------------------------------------------------------------------------
            // CONTEXT CONSTRUCTION: Build execution context for syscall
            // -------------------------------------------------------------------------
            let ctx = SyscallContext::new(call_id, cwd, cancel.clone())
                .with_actor(actor)
                .with_deadline(deadline_ms)
                .with_vfs_cwd(vfs_cwd);

            let timeout = deadline_ms.map(Duration::from_millis);

            // -------------------------------------------------------------------------
            // LANE EXECUTION: Acquire lane lock if needed, invoke syscall
            // WHY lane lock: Serialize mutations to prevent concurrent modification
            // of shared state (need queue, task queue, room state).
            // -------------------------------------------------------------------------
            let run = async {
                match lane {
                    Lane::Immediate => handler.execute(&ctx, data, inner_tx.clone()).await,
                    Lane::Need => {
                        let _g = need_lane.lock().await;
                        handler.execute(&ctx, data, inner_tx.clone()).await
                    }
                    Lane::Room => {
                        let _g = room_lane.lock().await;
                        handler.execute(&ctx, data, inner_tx.clone()).await
                    }
                }
            };

            // -------------------------------------------------------------------------
            // CANCELLATION AND TIMEOUT: Enforce deadline and handle cancellation
            // WHY select: Allows syscall to be interrupted by cancellation or deadline
            // even if it's blocked on I/O or computation.
            // -------------------------------------------------------------------------
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

            // -------------------------------------------------------------------------
            // ERROR HANDLING: Emit error frame if syscall returned Err
            // WHY: Separates protocol (error frame) from execution (Result error).
            // Syscall can emit ok/item frames before returning Err for cleanup.
            // -------------------------------------------------------------------------
            if let Err(e) = result {
                warn!(code = %e.code, "kernel error");
                let _ = inner_tx.send(Frame::error(call_id, e.to_value())).await;
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
