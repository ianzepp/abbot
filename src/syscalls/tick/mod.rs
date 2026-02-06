mod subscribe;

pub use subscribe::TickSubscribe;

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(TickSubscribe::new()));
}
