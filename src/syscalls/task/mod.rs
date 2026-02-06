mod complete;
mod enqueue;
mod lease;
mod list;
mod read;
mod search;
mod status;

pub use complete::TaskComplete;
pub use enqueue::TaskEnqueue;
pub use lease::TaskLease;
pub use list::TaskList;
pub use read::TaskRead;
pub use search::TaskSearch;
pub use status::TaskStatusGet;

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(TaskEnqueue::new()));
    dispatcher.register(Arc::new(TaskLease::new()));
    dispatcher.register(Arc::new(TaskComplete::new()));
    dispatcher.register(Arc::new(TaskStatusGet::new()));
    dispatcher.register(Arc::new(TaskList::new()));
    dispatcher.register(Arc::new(TaskRead::new()));
    dispatcher.register(Arc::new(TaskSearch::new()));
}
