mod update;

pub use update::LtmUpdate;

use std::sync::Arc;
use crate::kernel::KernelDispatcher;

pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(LtmUpdate::new()));
}
