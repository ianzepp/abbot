use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::kernel::KernelDispatcher;
use crate::syscalls;

static KERNEL: std::sync::OnceLock<Arc<Kernel>> = std::sync::OnceLock::new();

pub struct Kernel {
    dispatcher: RwLock<KernelDispatcher>,
    workspace_root: PathBuf,
}

impl Kernel {
    pub fn init(workspace_root: PathBuf) -> Arc<Self> {
        let kernel = Arc::new(Self::new(workspace_root));
        let _ = KERNEL.set(kernel.clone());
        tracing::info!("kernel initialized");
        kernel
    }

    pub fn get() -> Option<Arc<Self>> {
        KERNEL.get().cloned()
    }

    fn new(workspace_root: PathBuf) -> Self {
        let mut dispatcher = KernelDispatcher::new(workspace_root.clone());
        syscalls::register_all(&mut dispatcher);
        Self {
            dispatcher: RwLock::new(dispatcher),
            workspace_root,
        }
    }

    pub fn workspace_root(&self) -> &PathBuf {
        &self.workspace_root
    }

    pub async fn dispatcher(&self) -> tokio::sync::RwLockReadGuard<'_, KernelDispatcher> {
        self.dispatcher.read().await
    }

    pub async fn dispatcher_mut(&self) -> tokio::sync::RwLockWriteGuard<'_, KernelDispatcher> {
        self.dispatcher.write().await
    }
}
