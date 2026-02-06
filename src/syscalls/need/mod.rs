mod enqueue;
mod fulfill;
mod lease;

pub use enqueue::NeedEnqueue;
pub use fulfill::NeedFulfill;
pub use lease::NeedLease;

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(NeedEnqueue::new()));
    dispatcher.register(Arc::new(NeedLease::new()));
    dispatcher.register(Arc::new(NeedFulfill::new()));
}
