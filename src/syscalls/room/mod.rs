mod cancel;
mod create;
mod list;
mod reschedule;
mod run;
mod schedule;
mod stream;

pub use cancel::RoomCancel;
pub use create::RoomCreate;
pub use list::RoomList;
pub use reschedule::RoomReschedule;
pub use run::RoomRun;
pub use schedule::RoomSchedule;
pub use stream::RoomStream;

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(RoomCreate::new()));
    dispatcher.register(Arc::new(RoomStream::new()));
    dispatcher.register(Arc::new(RoomRun::new()));
    dispatcher.register(Arc::new(RoomSchedule::new()));
    dispatcher.register(Arc::new(RoomList::new()));
    dispatcher.register(Arc::new(RoomReschedule::new()));
    dispatcher.register(Arc::new(RoomCancel::new()));
}
