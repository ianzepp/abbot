//! Room - Multi-agent execution session management
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The room namespace provides syscalls for managing multi-agent collaboration
//! sessions where caller-defined agents execute in parallel rounds with shared
//! transcript synchronization.
//!
//! SYSCALL INVENTORY
//! =================
//! - `room:run` - Execute parallel agent loop (caller provides agents, prompt, config)
//! - `room:schedule` - Persist room schedule for deferred execution
//! - `room:list` - Query room schedules with optional filtering
//! - `room:reschedule` - Update run_after_ms for a pending schedule
//! - `room:cancel` - Mark a schedule as cancelled
//!
//! CONCURRENCY
//! ===========
//! All room operations execute on the Room lane (dedicated concurrency lane),
//! isolating long-running room sessions from Head/Hand/Mind execution.

mod cancel;
mod list;
mod reschedule;
mod run;
mod schedule;

pub use cancel::RoomCancel;
pub use list::RoomList;
pub use reschedule::RoomReschedule;
pub use run::RoomRun;
pub use schedule::RoomSchedule;

// =============================================================================
// REGISTRATION
// =============================================================================

/// Register all room syscalls with the kernel dispatcher.
pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(RoomRun::new()));
    dispatcher.register(Arc::new(RoomSchedule::new()));
    dispatcher.register(Arc::new(RoomList::new()));
    dispatcher.register(Arc::new(RoomReschedule::new()));
    dispatcher.register(Arc::new(RoomCancel::new()));
}
