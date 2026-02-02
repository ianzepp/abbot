pub mod fs;
pub mod git;
pub mod net;
pub mod proc;

pub use fs::{FsRead, FsWrite};
pub use git::GitRun;
pub use net::NetFetch;
pub use proc::ProcRun;

use std::sync::Arc;

use crate::kernel::KernelDispatcher;

/// Registers all standard syscalls with the dispatcher.
pub fn register_all(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(FsRead::new()));
    dispatcher.register(Arc::new(FsWrite::new()));
    dispatcher.register(Arc::new(ProcRun::new()));
    dispatcher.register(Arc::new(NetFetch::new()));
    dispatcher.register(Arc::new(GitRun::new()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_register_all() {
        let mut dispatcher = KernelDispatcher::new(PathBuf::from("/tmp"));
        register_all(&mut dispatcher);

        assert!(dispatcher.has("fs:read"));
        assert!(dispatcher.has("fs:write"));
        assert!(dispatcher.has("proc:run"));
        assert!(dispatcher.has("net:fetch"));
        assert!(dispatcher.has("git:run"));
    }
}
