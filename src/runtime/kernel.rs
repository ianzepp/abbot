use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use tokio::sync::RwLock;

use crate::kernel::{Frame, KernelDispatcher};
use crate::kernel::ExternalToolManager;
use crate::kernel::ReplyStreamManager;
use crate::kernel::NeedKernel;
use crate::kernel::TaskKernel;
use crate::kernel::RoomKernel;
use crate::kernel::TickKernel;
use crate::kernel::AuditLog;
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
    tick: std::sync::OnceLock<TickKernel>,
    workspace: PathBuf,
    store: std::sync::OnceLock<Arc<Store>>,
    activity_seq: AtomicU64,
    activity_last_ms: AtomicI64,
    audit: std::sync::OnceLock<Arc<AuditLog>>,
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

        // Start kernel tick clock.
        if let Some(k) = Kernel::get() {
            k.start_tick();
        }
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
            tick: std::sync::OnceLock::new(),
            workspace,
            store: std::sync::OnceLock::new(),
            activity_seq: AtomicU64::new(0),
            activity_last_ms: AtomicI64::new(now_ms()),
            audit: std::sync::OnceLock::new(),
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

    pub async fn subscribe_frames(&self) -> tokio::sync::broadcast::Receiver<Frame> {
        self.dispatcher.read().await.subscribe()
    }

    pub async fn set_audit(&self, audit: Arc<AuditLog>) {
        if self.audit.set(audit.clone()).is_ok() {
            let mut d = self.dispatcher_mut().await;
            d.set_audit(audit);
        }
    }

    pub fn audit(&self) -> Option<Arc<AuditLog>> {
        self.audit.get().cloned()
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

    pub fn start_tick(&self) {
        let interval_ms = std::env::var("KERNEL_TICK_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(1000);

        if self.tick.get().is_some() {
            return;
        }

        let interval = std::time::Duration::from_millis(interval_ms);
        let (tick, _rx) = TickKernel::new();
        tick.start(interval);
        let _ = self.tick.set(tick);
    }

    pub fn tick(&self) -> Option<&TickKernel> {
        self.tick.get()
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
