use std::sync::Arc;

use tokio::sync::RwLock;

use crate::kernel::KernelDispatcher;
use crate::syscalls;
use crate::vfs::MountTable;

use super::app_config::AppConfig;

static KERNEL: std::sync::OnceLock<Arc<Kernel>> = std::sync::OnceLock::new();

pub struct Kernel {
    dispatcher: RwLock<KernelDispatcher>,
}

impl Kernel {
    pub fn init() -> Arc<Self> {
        let config = AppConfig::global();
        if let Err(e) = MountTable::init(config.vfs.mounts.clone()) {
            tracing::warn!(error = %e, "failed to initialize VFS mount table");
        }

        let kernel = Arc::new(Self::new());
        let _ = KERNEL.set(kernel.clone());
        tracing::info!("kernel initialized");
        kernel
    }

    pub fn get() -> Option<Arc<Self>> {
        KERNEL.get().cloned()
    }

    fn new() -> Self {
        let mut dispatcher = KernelDispatcher::new();
        syscalls::register_all(&mut dispatcher);
        Self {
            dispatcher: RwLock::new(dispatcher),
        }
    }

    pub async fn dispatcher(&self) -> tokio::sync::RwLockReadGuard<'_, KernelDispatcher> {
        self.dispatcher.read().await
    }

    pub async fn dispatcher_mut(&self) -> tokio::sync::RwLockWriteGuard<'_, KernelDispatcher> {
        self.dispatcher.write().await
    }
}
