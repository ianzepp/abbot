mod model_set;

pub use model_set::SessionModelSet;

use std::sync::Arc;
use crate::kernel::KernelDispatcher;

pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(SessionModelSet::new()));
}
