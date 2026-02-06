//! Tick - Heartbeat subscription and kernel idle detection system
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The `tick` namespace provides a subscription-based heartbeat system that enables agents and
//! services to observe kernel lifecycle events and idle state transitions. The tick service emits
//! periodic SIGTICK events containing sequence numbers and timing information that coordinators
//! use to schedule background work (room deliberations, maintenance tasks).
//!
//! **Core components:**
//! - `TickService` - Kernel-wide heartbeat generator (started during kernel initialization)
//! - `tick:subscribe` - Subscribe to tick events via tokio::sync::watch channel
//! - `SIGTICK` events - Periodic frames with seq (sequence number), now_ms (timestamp), dt_ms (delta)
//! - `RoomCoordinator` integration - Primary consumer of tick events for idle detection
//!
//! **Tick lifecycle:**
//! 1. Kernel starts TickService as background task (Arc<TickService>.start())
//! 2. TickService emits periodic ticks via tokio::sync::watch channel
//! 3. Subscribers call tick:subscribe to receive current tick + future updates
//! 4. RoomCoordinator uses tick deltas to detect slow_idle and deep_idle states
//! 5. Idle states trigger room scheduling (autonomy rooms on slow_idle, conclaves on deep_idle)
//!
//! **Integration points:**
//! - `Kernel::tick()` - Global accessor for TickService instance
//! - `RoomCoordinator` - Subscribes to ticks for idle detection
//! - `activity_tracker` - Kernel activity bumps reset idle detection
//! - Frame protocol - SIGTICK events emitted as Frame::event
//!
//! **Registered syscalls:**
//! - `tick:subscribe` - Subscribe to heartbeat events (long-running stream)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Push-based subscription**: Ticks are broadcast, not polled (efficient idle waiting)
//! - **Immediate delivery**: Subscription emits current tick immediately (no startup delay)
//! - **Sequence numbering**: Each tick has monotonic sequence number for ordering/deduplication
//! - **Delta timing**: dt_ms enables measuring time since last tick (idle detection)
//! - **Cancellation-aware**: Subscription loop terminates on ctx.cancel
//! - **Multi-subscriber support**: tokio::sync::watch enables many concurrent subscribers
//!
//! WHY TICK:SUBSCRIBE EXISTS
//! =========================
//! Kernel needs a way to schedule background work during idle periods without polling:
//!
//! **Without tick subscription (polling approach):**
//! - RoomCoordinator must poll kernel activity state every N milliseconds
//! - Wastes CPU cycles checking idle state when kernel is busy
//! - Delay between idle detection and room scheduling (polling interval)
//!
//! **With tick subscription (push approach):**
//! - RoomCoordinator blocks efficiently on tick stream (no CPU waste)
//! - Immediate notification when tick occurs (no polling delay)
//! - Centralized heartbeat (all idle detection uses same tick source)
//!
//! SIGTICK EVENT FORMAT
//! ====================
//! Each tick event is a Frame::event with the following JSON payload:
//!
//! ```json
//! {
//!   "kind": "SIGTICK",
//!   "seq": 42,              // Monotonic sequence number (starts at 1)
//!   "now_ms": 1704067200000, // Current timestamp in milliseconds since epoch
//!   "dt_ms": 100            // Milliseconds since last tick
//! }
//! ```
//!
//! **Field semantics:**
//! - `seq` - Monotonic counter incremented per tick (enables deduplication)
//! - `now_ms` - Absolute timestamp for correlation with other events
//! - `dt_ms` - Delta time since previous tick (idle detection metric)
//!
//! **WHY dt_ms instead of fixed interval:**
//! TickService emits ticks at variable intervals based on kernel activity:
//! - Fast ticks (~50-100ms) when kernel is busy
//! - Slow ticks (~500ms-1s) when kernel is idle
//! - Deep idle ticks (~2-5s) when kernel has been idle for extended period
//!
//! dt_ms enables subscribers to detect idle state transitions without tracking
//! tick history (single event contains all necessary timing information).
//!
//! IDLE DETECTION INTEGRATION
//! ===========================
//! RoomCoordinator uses tick subscription for idle-based room scheduling:
//!
//! **Idle state transitions:**
//! 1. **Active** (dt_ms < slow_idle_threshold)
//!    - Kernel processing user requests or agent tasks
//!    - No room scheduling (agents are busy)
//!
//! 2. **Slow idle** (slow_idle_threshold < dt_ms < deep_idle_threshold)
//!    - Kernel has been idle for minutes
//!    - Schedule autonomy room (quick 3-round reflection)
//!
//! 3. **Deep idle** (dt_ms > deep_idle_threshold)
//!    - Kernel has been idle for longer period
//!    - Schedule conclave room (strategic 5-round planning)
//!
//! **Activity bumps:**
//! Kernel activity (syscall dispatch, frame logging) resets idle timer via
//! `Kernel::bump_activity()`. This prevents room scheduling when kernel is
//! actively processing work, even if ticks are slow.
//!
//! CONCURRENCY
//! ===========
//! - **TickService**: Single background task emits ticks via tokio::sync::watch
//! - **Subscribers**: Multiple concurrent tick:subscribe calls (each gets independent receiver)
//! - **Watch channel**: Tokio::sync::watch is multi-producer-single-consumer broadcast
//! - **No contention**: Subscribers don't block each other (independent receivers)
//!
//! PERFORMANCE
//! ===========
//! - Tick subscription overhead: <1μs per tick (watch channel read)
//! - Frame serialization: ~10-50μs per SIGTICK event (JSON encoding)
//! - Memory: O(1) per subscriber (single watch receiver, no buffering)
//! - No I/O: Ticks are in-memory only (no disk/network operations)
//!
//! SECURITY MODEL
//! ==============
//! **No actor restrictions:**
//! - WHY: Tick events are non-sensitive timing information
//! - All agents (head, hand, room) can subscribe to ticks
//! - No mutation guard required (read-only subscription)
//!
//! **No data leakage:**
//! - SIGTICK events contain no user data or secrets
//! - Only timing metadata (sequence number, timestamps)
//! - Safe to expose to all agents without authorization
//!
//! TRADE-OFFS
//! ==========
//! 1. **Watch Channel vs. Broadcast Channel**
//!    - CHOSEN: tokio::sync::watch (latest-only semantics)
//!    - REJECTED: tokio::sync::broadcast (queue-based delivery)
//!    - WHY: Subscribers only care about current tick state (not historical ticks)
//!    - IMPLICATION: Slow subscribers skip ticks (acceptable for idle detection)
//!
//! 2. **Immediate Delivery vs. Wait for Next Tick**
//!    - CHOSEN: Emit current tick immediately on subscription
//!    - WHY: Subscribers get instant feedback (no waiting for next tick)
//!    - IMPLICATION: Subscribers may receive same tick twice (seq deduplication handles this)
//!
//! 3. **Variable Tick Rate vs. Fixed Interval**
//!    - CHOSEN: Variable tick rate based on kernel activity
//!    - WHY: Reduces CPU waste during idle periods, faster ticks when busy
//!    - IMPLICATION: Subscribers must use dt_ms (not assume fixed interval)

mod subscribe;

pub use subscribe::TickSubscribe;

use std::sync::Arc;
use crate::kernel::KernelDispatcher;

/// Register all tick namespace syscalls with the kernel dispatcher.
///
/// WHY: Centralizes syscall registration for the tick namespace. Currently
/// only registers `tick:subscribe`, but structured for future expansion
/// (e.g., tick:status, tick:configure).
///
/// REGISTERED SYSCALLS:
/// - `tick:subscribe` - Subscribe to heartbeat events (long-running stream)
pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(TickSubscribe::new()));
}
