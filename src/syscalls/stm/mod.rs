pub mod read;
mod update;

pub use read::StmRead;
pub use update::StmUpdate;

use std::sync::Arc;
use crate::kernel::KernelDispatcher;

pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(StmRead::new()));
    dispatcher.register(Arc::new(StmUpdate::new()));
}
