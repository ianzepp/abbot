//! Syscall Module - Central registration point for all syscalls
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module declares all syscall namespaces and provides a unified registration
//! function for the kernel dispatcher. The syscall refactor (see syscall-refactor-spec.md)
//! established clear namespace boundaries (`chat:*`, `llm:*`, `room:*`, etc.) to
//! separate concerns and eliminate legacy frame flow ambiguities.
//!
//! WHY namespaces: Prevents naming collisions, makes syscall purpose explicit by
//! inspection, and enables routing/lane assignment based on namespace prefix.

pub mod fs;
pub mod git;
pub mod chat;
pub mod llm;
pub mod frames;
pub mod need;
pub mod net;
pub mod proc;
pub mod room;
pub mod task;
pub mod tick;
pub mod tool;

pub use fs::{FsRead, FsWrite};
pub use git::GitRun;
pub use net::NetFetch;
pub use proc::ProcRun;

use std::sync::Arc;

use crate::kernel::KernelDispatcher;

/// Registers all standard syscalls with the dispatcher.
///
/// WHY centralized: Ensures every syscall namespace is loaded exactly once and
/// makes the full syscall surface area visible at a glance.
pub fn register_all(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(FsRead::new()));
    dispatcher.register(Arc::new(FsWrite::new()));
    dispatcher.register(Arc::new(ProcRun::new()));
    dispatcher.register(Arc::new(NetFetch::new()));
    dispatcher.register(Arc::new(GitRun::new()));
    chat::register(dispatcher);
    llm::register(dispatcher);
    frames::register(dispatcher);
    need::register(dispatcher);
    task::register(dispatcher);
    room::register(dispatcher);
    tick::register(dispatcher);
    tool::register(dispatcher);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_all() {
        let mut dispatcher = KernelDispatcher::new();
        register_all(&mut dispatcher);

        assert!(dispatcher.has("fs:read"));
        assert!(dispatcher.has("fs:write"));
        assert!(dispatcher.has("proc:run"));
        assert!(dispatcher.has("net:fetch"));
        assert!(dispatcher.has("git:run"));
    }
}
