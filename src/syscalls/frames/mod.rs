mod append;
mod select;

pub use append::FramesAppend;
pub use select::FramesSelect;

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(FramesAppend::new()));
    dispatcher.register(Arc::new(FramesSelect::new()));
}
