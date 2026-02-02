use std::path::Path;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::kernel::KernelDispatcher;
use crate::kernel::ExternalToolManager;
use crate::kernel::ReplyStreamManager;
use crate::kernel::NeedKernel;
use crate::kernel::TaskKernel;
use crate::syscalls;
use crate::vfs::{MountConfig, MountMode, MountTable};

use super::app_config::AppConfig;

static KERNEL: std::sync::OnceLock<Arc<Kernel>> = std::sync::OnceLock::new();

pub struct Kernel {
    dispatcher: RwLock<KernelDispatcher>,
    external_tools: ExternalToolManager,
    reply_streams: ReplyStreamManager,
    needs: NeedKernel,
    tasks: TaskKernel,
}

impl Kernel {
    /// Initialize the kernel with the given workspace path.
    /// Auto-mounts `<workspace>/root` at `/` unless config has a root mount override.
    pub fn init(workspace: &Path) -> Arc<Self> {
        let config = AppConfig::global();
        let mut mounts = Vec::new();

        // Check if config already has a root mount override
        let has_root_override = config.vfs.mounts.iter().any(|m| m.prefix == "/");

        if !has_root_override {
            // Auto-mount workspace/root at /
            let root_path = workspace.join("root");
            mounts.push(MountConfig {
                prefix: "/".to_string(),
                host: root_path.to_string_lossy().to_string(),
                mode: MountMode::Rw,
            });
            tracing::info!(host = %root_path.display(), "auto-mounted workspace/root at /");
        }

        // Add config mounts after auto-mount
        mounts.extend(config.vfs.mounts.clone());

        if let Err(e) = MountTable::init(mounts) {
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
            external_tools: ExternalToolManager::new(),
            reply_streams: ReplyStreamManager::new(),
            needs: NeedKernel::new(),
            tasks: TaskKernel::new(),
        }
    }

    pub async fn dispatcher(&self) -> tokio::sync::RwLockReadGuard<'_, KernelDispatcher> {
        self.dispatcher.read().await
    }

    pub async fn dispatcher_mut(&self) -> tokio::sync::RwLockWriteGuard<'_, KernelDispatcher> {
        self.dispatcher.write().await
    }

    pub fn external_tools(&self) -> &ExternalToolManager {
        &self.external_tools
    }

    pub fn reply_streams(&self) -> &ReplyStreamManager {
        &self.reply_streams
    }

    pub fn needs(&self) -> &NeedKernel {
        &self.needs
    }

    pub fn tasks(&self) -> &TaskKernel {
        &self.tasks
    }
}
