//! Tick:Subscribe - Stream periodic clock events
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Subscribes to the kernel's global tick service and streams timing events to the
//! caller. Each event contains a monotonic sequence number, current timestamp, and
//! delta since the last tick.
//!
//! **Integration:** Connects to `TickService` (via `Kernel::tick()`) which broadcasts
//! timing updates through a tokio watch channel. The syscall converts watch events into
//! `Frame::event` emissions on the caller's frame channel.
//!
//! **Frame protocol:**
//! - Emits `Frame::event` with `{"kind": "SIGTICK", "seq": N, "now_ms": T, "dt_ms": D}` on each tick
//! - Sends immediate event with current tick upon subscription
//! - Emits `Frame::done` when subscription ends (caller cancels or tick service stops)
//! - Returns `E_INTERNAL` if kernel or tick service is not initialized
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Immediate feedback**: Send current tick immediately so caller doesn't wait for first update
//! - **Cancellation hygiene**: Clean exit on cancellation without orphaning resources
//! - **Monotonic sequence**: Sequence numbers enable detection of missed ticks
//! - **Long-lived subscription**: Runs indefinitely until cancelled (no timeout)
//!
//! CONCURRENCY
//! ===========
//! - Uses `tokio::select!` to race between cancellation and tick events
//! - Watch channel ensures latest tick value is always available (no buffering)
//! - Safe for multiple concurrent subscribers (watch channel broadcasts to all)
//!
//! USE CASES
//! =========
//! - **Animation loops**: Update UI or state machines at consistent frame rates
//! - **Timeout implementation**: Wait for events with time-based fallback
//! - **Periodic tasks**: Execute operations every N ticks
//! - **Latency monitoring**: Track dt_ms to detect clock jitter or scheduling delays

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for subscribing to kernel tick events.
///
/// WHY: Stateless unit struct since tick subscription requires no configuration.
pub struct TickSubscribe;

impl Default for TickSubscribe {
    fn default() -> Self {
        Self::new()
    }
}

impl TickSubscribe {
    /// Create a new `TickSubscribe` syscall.
    ///
    /// WHY: Standard constructor for consistency with other syscalls.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TickSubscribe {
    fn name(&self) -> &'static str {
        "tick:subscribe"
    }

    /// Subscribe to periodic clock events from the kernel tick service.
    ///
    /// WHY: Enables time-based coordination across agents without polling. The
    /// global tick service provides a single source of truth for timing, preventing
    /// clock skew between concurrent operations.
    ///
    /// USE CASE: Invoked by agents that need periodic updates (animations, timeouts,
    /// polling loops). Runs indefinitely until caller cancels or tick service stops.
    ///
    /// CONCURRENCY: Uses `tokio::select!` to race cancellation against tick events,
    /// ensuring clean shutdown without orphaning the subscription.
    ///
    /// RETURNS:
    /// - `Frame::event` with `{"kind": "SIGTICK", "seq": N, "now_ms": T, "dt_ms": D}` on each tick
    /// - `Frame::done` when subscription ends
    /// - `E_INTERNAL` if kernel or tick service is not initialized
    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // WHY: Kernel must be initialized for tick service access
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        // WHY: Tick service is optional (not all kernel configs enable it)
        let Some(tick) = k.tick() else {
            return Err(KernelError::internal("kernel tick not initialized"));
        };

        // =====================================================================
        // PHASE 1: Initial Tick Delivery
        // =====================================================================
        // WHY: Send current tick immediately so caller doesn't wait for next update.
        // This provides instant feedback and establishes baseline timing.
        let mut rx = tick.receiver();
        {
            let cur = rx.borrow().clone();
            let _ = tx
                .send(Frame::event(
                    ctx.call_id,
                    json!({"kind": "SIGTICK", "seq": cur.seq, "now_ms": cur.now_ms, "dt_ms": cur.dt_ms}),
                ))
                .await;
        }

        // =====================================================================
        // PHASE 2: Tick Streaming Loop
        // =====================================================================
        // WHY: Watch for tick updates or cancellation. Exit cleanly on either event
        // to prevent orphaned subscriptions or hung tasks.
        loop {
            tokio::select! {
                // WHY: Cancellation takes priority - exit immediately to avoid sending
                // more events after caller has disconnected
                _ = ctx.cancel.cancelled() => {
                    break;
                }

                // WHY: Watch channel signals when tick value changes. Err indicates
                // sender dropped (tick service stopped), so exit gracefully
                changed = rx.changed() => {
                    if changed.is_err() {
                        break;
                    }

                    let cur = rx.borrow().clone();

                    // WHY: Send error breaks loop since caller has disconnected.
                    // No point continuing to monitor ticks if no one is listening.
                    if tx
                        .send(Frame::event(
                            ctx.call_id,
                            json!({"kind": "SIGTICK", "seq": cur.seq, "now_ms": cur.now_ms, "dt_ms": cur.dt_ms}),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }

        // WHY: Signal end of subscription with Frame::done. Ignoring send error is
        // safe since caller may have already disconnected
        let _ = tx.send(Frame::done(ctx.call_id)).await;
        Ok(())
    }
}
