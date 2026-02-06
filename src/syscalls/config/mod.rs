mod read;
mod update;

pub use read::ConfigRead;
pub use update::ConfigUpdate;

use std::sync::Arc;
use crate::kernel::KernelDispatcher;

pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(ConfigRead::new()));
    dispatcher.register(Arc::new(ConfigUpdate::new()));
}
