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

pub mod dispatch;
pub mod docs;
pub mod fs;
pub mod git;
pub mod chat;
pub mod llm;
pub mod frames;
pub mod need;
pub mod net;
pub mod exec;
pub mod room;
pub mod task;
pub mod config;
pub mod ltm;
pub mod models;
pub mod patch;
pub mod session;
pub mod state;
pub mod stm;
pub mod text;
pub mod tick;
pub mod tool;
pub mod want;

pub use fs::{FsDiff, FsList, FsMkdir, FsRead, FsSearch, FsWrite};
pub use git::GitRun;
pub use net::NetFetch;
pub use exec::ExecRun;

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
    dispatcher.register(Arc::new(FsSearch::new()));
    dispatcher.register(Arc::new(FsMkdir::new()));
    dispatcher.register(Arc::new(FsDiff::new()));
    dispatcher.register(Arc::new(ExecRun::new()));
    dispatcher.register(Arc::new(NetFetch::new()));
    dispatcher.register(Arc::new(GitRun::new()));
    chat::register(dispatcher);
    docs::register(dispatcher);
    llm::register(dispatcher);
    frames::register(dispatcher);
    need::register(dispatcher);
    task::register(dispatcher);
    room::register(dispatcher);
    config::register(dispatcher);
    models::register(dispatcher);
    patch::register(dispatcher);
    session::register(dispatcher);
    state::register(dispatcher);
    stm::register(dispatcher);
    text::register(dispatcher);
    ltm::register(dispatcher);
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
        assert!(dispatcher.has("git:run"));
        assert!(dispatcher.has("fs:list"));
        assert!(dispatcher.has("fs:search"));
        assert!(dispatcher.has("fs:mkdir"));
        assert!(dispatcher.has("fs:diff"));
        assert!(dispatcher.has("patch:apply"));
        assert!(dispatcher.has("text:echo"));
        assert!(dispatcher.has("config:read"));
        assert!(dispatcher.has("config:update"));
        assert!(dispatcher.has("models:list"));
        assert!(dispatcher.has("session:model_set"));
        assert!(dispatcher.has("state:query"));
        assert!(dispatcher.has("stm:read"));
        assert!(dispatcher.has("stm:update"));
        assert!(dispatcher.has("tool:explain"));
        assert!(dispatcher.has("task:list"));
        assert!(dispatcher.has("task:read"));
        assert!(dispatcher.has("task:search"));
        assert!(dispatcher.has("docs:list"));
        assert!(dispatcher.has("docs:search"));
        assert!(dispatcher.has("docs:read"));
    }
}
