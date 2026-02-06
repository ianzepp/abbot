//! Kernel Router - Lane assignment for syscall concurrency control
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The router assigns syscalls to concurrency lanes to prevent deadlocks while
//! preserving ordering guarantees for stateful operations. Lanes are mutex-
//! protected execution queues in the dispatcher.
//!
//! WHY lanes exist: Some syscalls mutate shared state (need/task queues, room
//! state) and require serialization. Others are read-only or long-polling and
//! should run concurrently. Lane assignment prevents deadlock (lease blocking
//! enqueue) while maintaining safety (need mutations serialized).

/// Concurrency lane for syscall execution.
///
/// WHY these lanes:
/// - Immediate: No shared state, run concurrently (chat:*, lease, read-only ops)
/// - Need: Serialize need queue mutations (enqueue, fulfill, cancel)
/// - Task: Serialize task queue mutations (enqueue, complete, cancel)
/// - Room: Serialize room state mutations (create, join, leave, mind ops)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Immediate,
    Need,
    Task,
    Room,
}

/// Syscall lane router.
///
/// WHY: Centralizes lane assignment logic for syscall dispatch.
#[derive(Debug, Default, Clone)]
pub struct KernelRouter;

impl KernelRouter {
    pub fn new() -> Self {
        Self
    }

    /// Assign a syscall to a concurrency lane.
    ///
    /// WHY lane assignment rules:
    /// - Lease calls are long-polling and MUST NOT share a lane with enqueue/
    ///   complete to avoid deadlock (lease holds lane lock while waiting).
    /// - chat:* syscalls are side-effecting emitters and run immediately.
    /// - need:*, task:*, room:* namespace prefixes map to their respective
    ///   serialization lanes to prevent concurrent mutation of shared state.
    pub fn lane_for(&self, syscall_name: &str) -> Lane {
        // WHY immediate for lease: Lease blocks waiting for work; if it shares
        // a lane with enqueue, deadlock occurs (lease holds lock, enqueue waits).
        if syscall_name == "need:lease" || syscall_name == "task:lease" {
            return Lane::Immediate;
        }

        // WHY immediate for chat: chat:* syscalls emit to turn streams (no shared
        // state mutations that require serialization).
        if syscall_name.starts_with("chat:") {
            return Lane::Immediate;
        }

        // WHY serialize task/need/room operations: Prevent concurrent mutations
        // of queue/room state.
        if syscall_name.starts_with("task:") {
            return Lane::Task;
        }
        if syscall_name.starts_with("need:") {
            return Lane::Need;
        }

        // WHY immediate for room:list: Read-only query against SQLite, no shared
        // state mutation. Putting it on the Room lane would block behind running rooms.
        if syscall_name == "room:list" {
            return Lane::Immediate;
        }

        if syscall_name.starts_with("room:") {
            return Lane::Room;
        }

        Lane::Immediate
    }
}
