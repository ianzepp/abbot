mod create;
mod list;
mod promote;
mod remove;

pub use create::WantCreate;
pub use list::WantList;
pub use promote::WantPromote;
pub use remove::WantRemove;

use std::sync::Arc;
use crate::kernel::KernelDispatcher;

pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(WantList::new()));
    dispatcher.register(Arc::new(WantCreate::new()));
    dispatcher.register(Arc::new(WantRemove::new()));
    dispatcher.register(Arc::new(WantPromote::new()));
}
