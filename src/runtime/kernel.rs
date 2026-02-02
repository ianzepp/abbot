use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use tokio::sync::RwLock;

use crate::kernel::KernelDispatcher;
use crate::kernel::ExternalToolManager;
use crate::kernel::ReplyStreamManager;
use crate::kernel::NeedKernel;
use crate::kernel::TaskKernel;
use crate::kernel::RoomKernel;
use crate::syscalls;
use crate::vfs::{MountConfig, MountMode, MountTable};
use crate::history::Store;

use super::app_config::AppConfig;

static KERNEL: std::sync::OnceLock<Arc<Kernel>> = std::sync::OnceLock::new();

pub struct Kernel {
    dispatcher: RwLock<KernelDispatcher>,
    external_tools: ExternalToolManager,
    reply_streams: ReplyStreamManager,
    needs: NeedKernel,
    tasks: TaskKernel,
    rooms: RoomKernel,
    workspace: PathBuf,
    store: std::sync::OnceLock<Arc<Store>>,
    activity_seq: AtomicU64,
    activity_last_ms: AtomicI64,
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

        let kernel = Arc::new(Self::new(workspace.to_path_buf()));
        let _ = KERNEL.set(kernel.clone());
        tracing::info!("kernel initialized");
        kernel
    }

    pub fn get() -> Option<Arc<Self>> {
        KERNEL.get().cloned()
    }

    fn new(workspace: PathBuf) -> Self {
        let mut dispatcher = KernelDispatcher::new();
        syscalls::register_all(&mut dispatcher);
        Self {
            dispatcher: RwLock::new(dispatcher),
            external_tools: ExternalToolManager::new(),
            reply_streams: ReplyStreamManager::new(),
            needs: NeedKernel::new(),
            tasks: TaskKernel::new(),
            rooms: RoomKernel::new(),
            workspace,
            store: std::sync::OnceLock::new(),
            activity_seq: AtomicU64::new(0),
            activity_last_ms: AtomicI64::new(now_ms()),
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn set_store(&self, store: Arc<Store>) {
        let _ = self.store.set(store);
    }

    pub fn store(&self) -> Option<Arc<Store>> {
        self.store.get().cloned()
    }

    pub fn bump_activity(&self) -> u64 {
        self.activity_last_ms.store(now_ms(), Ordering::Relaxed);
        self.activity_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn activity_seq(&self) -> u64 {
        self.activity_seq.load(Ordering::Relaxed)
    }

    pub fn activity_last_ms(&self) -> i64 {
        self.activity_last_ms.load(Ordering::Relaxed)
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

    pub fn rooms(&self) -> &RoomKernel {
        &self.rooms
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
