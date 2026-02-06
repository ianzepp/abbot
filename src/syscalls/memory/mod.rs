mod recall;

pub use recall::MemoryRecall;

use std::sync::Arc;
use crate::kernel::KernelDispatcher;

pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(MemoryRecall::new()));
}

pub fn register_with_search(
    dispatcher: &mut KernelDispatcher,
    search: Arc<crate::recall::Search>,
) {
    dispatcher.register(Arc::new(MemoryRecall::with_search(search)));
}
