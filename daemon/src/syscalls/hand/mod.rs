//! Hand Namespace - Direct hand agent invocation
//!
//! Provides the `hand:run` syscall for directly invoking a hand's LLM+tool
//! loop without going through the task queue. This enables room agents and
//! other callers to dispatch focused work tasks synchronously.

mod run;

pub use run::HandRun;

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(HandRun::new()));
}
