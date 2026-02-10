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

pub mod chat;
pub mod dispatch;
pub mod docs;
pub mod ems;
pub mod exec;
pub mod frames;
pub mod fs;
pub mod hand;
pub mod llm;
pub mod need;
pub mod net;
pub mod patch;
pub mod room;
pub mod session;
pub mod tick;
pub mod tool;
pub mod traits;
pub mod want;

pub use exec::{EXEC_DEFAULT, ExecRun};
pub use fs::{FsCd, FsGrep, FsList, FsMkdir, FsRead, FsWrite};
pub use net::NetFetch;

use std::sync::Arc;

use crate::kernel::KernelDispatcher;

/// Registers all standard syscalls with the dispatcher.
///
/// WHY centralized: Ensures every syscall namespace is loaded exactly once and
/// makes the full syscall surface area visible at a glance.
pub fn register_all(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(FsRead::new()));
    dispatcher.register(Arc::new(FsWrite::new()));
    dispatcher.register(Arc::new(FsList::new()));
    dispatcher.register(Arc::new(FsGrep::new()));
    dispatcher.register(Arc::new(FsMkdir::new()));
    dispatcher.register(Arc::new(FsCd::new()));
    dispatcher.register(Arc::new(ExecRun::from_config()));
    dispatcher.register(Arc::new(NetFetch::new()));
    chat::register(dispatcher);
    docs::register(dispatcher);
    hand::register(dispatcher);
    llm::register(dispatcher);
    frames::register(dispatcher);
    need::register(dispatcher);
    room::register(dispatcher);
    patch::register(dispatcher);
    session::register(dispatcher);
    ems::register(dispatcher);
    traits::register(dispatcher);
    want::register(dispatcher);
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
        assert!(dispatcher.has("exec:run"));
        assert!(dispatcher.has("net:fetch"));
        assert!(dispatcher.has("fs:list"));
        assert!(dispatcher.has("fs:grep"));
        assert!(dispatcher.has("fs:mkdir"));
        assert!(dispatcher.has("fs:cd"));
        assert!(dispatcher.has("patch:apply"));
        assert!(dispatcher.has("session:model_set"));
        assert!(dispatcher.has("tool:explain"));
        assert!(dispatcher.has("docs:list"));
        assert!(dispatcher.has("docs:search"));
        assert!(dispatcher.has("docs:read"));
        assert!(dispatcher.has("ems:list"));
        assert!(dispatcher.has("ems:insert"));
        assert!(dispatcher.has("ems:select"));
        assert!(dispatcher.has("ems:update"));
        assert!(dispatcher.has("ems:delete"));
        assert!(dispatcher.has("ems:describe"));
        assert!(dispatcher.has("hand:run"));
        assert!(dispatcher.has("traits:list"));
        assert!(dispatcher.has("traits:describe"));
        assert!(dispatcher.has("traits:set"));
        assert!(dispatcher.has("traits:unset"));
    }
}
