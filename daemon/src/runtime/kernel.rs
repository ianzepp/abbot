//! Kernel - Global singleton coordinating syscalls, needs, tasks, turns, and VFS
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The Kernel is the single global coordinator for Abbot's runtime. It owns all
//! kernel-level subsystems (dispatcher, turn runtime, sigcall hub, need queue,
//! task queue, room registry, tick clock) and provides thread-safe access to them.
//!
//! The Kernel is initialized once at startup and accessed globally via `Kernel::get()`.
//! It automatically mounts the workspace's `root/` directory at VFS `/` and starts
//! the kernel tick clock for time-based operations.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Single source of truth: All kernel-level state is owned here
//! - Lazy subsystem access: Subsystems are accessed through getters, not passed around
//! - Automatic VFS setup: Workspace root is auto-mounted unless config overrides it
//! - Global singleton pattern: Kernel::get() provides access from anywhere in the runtime
//!
//! TRADE-OFFS
//! ==========
//! - Global state vs dependency injection: We chose global singleton for ergonomics.
//!   Syscalls and runtime services can access the kernel without threading it through
//!   every function signature. The cost is that testing requires initialization.
//! - VFS auto-mount: We auto-mount workspace/root at / for convenience. This means
//!   the VFS behavior depends on where the kernel was initialized, which could be
//!   surprising. The benefit is zero-config VFS for typical deployments.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use tokio::sync::RwLock;

use crate::history::Store;
use crate::kernel::FrameStore;
use crate::kernel::ExternalToolManager;
use crate::kernel::NeedKernel;
use crate::kernel::RoomKernel;
use crate::kernel::SigcallHub;
use crate::kernel::TaskKernel;
use crate::kernel::TickKernel;
use crate::kernel::TurnRuntime;
use crate::kernel::{Frame, KernelDispatcher};
use crate::syscalls;
use crate::vfs::{MountConfig, MountMode, MountTable};

use super::app_config::AppConfig;

// =============================================================================
// GLOBAL KERNEL SINGLETON
// =============================================================================
//
// WHY global singleton: Syscalls and runtime services need kernel access without
// passing it through every function signature. The kernel is initialized once
// at startup and never replaced, making OnceLock safe and ergonomic.

static KERNEL: std::sync::OnceLock<Arc<Kernel>> = std::sync::OnceLock::new();

// =============================================================================
// KERNEL STRUCTURE
// =============================================================================
//
// The Kernel owns all kernel-level subsystems as fields. Each subsystem is
// accessed through getter methods to maintain encapsulation.
//
// WHY RwLock for dispatcher: The dispatcher registry may be mutated at runtime
// (e.g., adding syscalls dynamically), so it requires interior mutability.
//
// WHY OnceLock for store/tick/audit: These are initialized after kernel creation
// and never replaced, making OnceLock appropriate.

pub struct Kernel {
    dispatcher: RwLock<KernelDispatcher>,
    external_tools: ExternalToolManager,
    turns: TurnRuntime,
    sigcalls: SigcallHub,
    needs: NeedKernel,
    tasks: TaskKernel,
    rooms: RoomKernel,
    tick: std::sync::OnceLock<TickKernel>,
    workspace: PathBuf,
    store: std::sync::OnceLock<Arc<Store>>,
    activity_seq: AtomicU64,
    activity_last_ms: AtomicI64,
    frames: std::sync::OnceLock<Arc<FrameStore>>,
}

// =============================================================================
// INITIALIZATION
// =============================================================================

impl Kernel {
    /// Initialize the kernel singleton with the given workspace path.
    ///
    /// WHY this initializes the global singleton: The kernel must be accessible
    /// from syscalls and runtime services without passing it through every call.
    /// Initialization happens once at startup before any syscalls are dispatched.
    ///
    /// SIDE EFFECTS:
    /// - Auto-mounts `<workspace>/root` at VFS `/` unless config overrides it
    /// - Registers all syscalls with the dispatcher
    /// - Starts the kernel tick clock for time-based operations
    pub fn init(workspace: &Path) -> Arc<Self> {
        // -------------------------------------------------------------------------
        // PHASE 1: VFS SETUP
        // WHY: The VFS must be initialized before any syscalls run, since syscalls
        // may read files. We auto-mount workspace/root at / for zero-config usage.
        // -------------------------------------------------------------------------
        let config = AppConfig::global();
        let mut mounts = Vec::new();

        let has_root_override = config.vfs.mounts.iter().any(|m| m.prefix == "/");

        if !has_root_override {
            let root_path = workspace.join("root");
            mounts.push(MountConfig {
                prefix: "/".to_string(),
                host: root_path.to_string_lossy().to_string(),
                mode: MountMode::Rw,
            });
            tracing::info!(host = %root_path.display(), "auto-mounted workspace/root at /");
        }

        mounts.extend(config.vfs.mounts.clone());

        if let Err(e) = MountTable::init(mounts) {
            tracing::warn!(error = %e, "failed to initialize VFS mount table");
        }

        // -------------------------------------------------------------------------
        // PHASE 2: KERNEL CREATION
        // WHY: Create the kernel instance and register it globally before starting
        // any subsystems, so that subsystems can call Kernel::get() if needed.
        // -------------------------------------------------------------------------
        let kernel = Arc::new(Self::new(workspace.to_path_buf()));
        let _ = KERNEL.set(kernel.clone());
        tracing::info!("kernel initialized");

        // -------------------------------------------------------------------------
        // PHASE 3: TICK CLOCK STARTUP
        // WHY: The tick clock drives time-based operations (e.g., mind wake cycles).
        // Start it after kernel registration so tick handlers can access the kernel.
        // -------------------------------------------------------------------------
        if let Some(k) = Kernel::get() {
            k.start_tick();
        }
        kernel
    }

    /// Get the global kernel singleton.
    ///
    /// WHY: Syscalls and runtime services need kernel access without dependency
    /// injection. Returns None only if called before Kernel::init().
    pub fn get() -> Option<Arc<Self>> {
        KERNEL.get().cloned()
    }

    /// Create a new kernel instance with all subsystems initialized.
    ///
    /// WHY private: Only Kernel::init() should create instances. This ensures
    /// the global singleton is the only kernel in the runtime.
    fn new(workspace: PathBuf) -> Self {
        let mut dispatcher = KernelDispatcher::new();
        syscalls::register_all(&mut dispatcher);
        let broadcast_tx = dispatcher.broadcast_sender(); // WHY: SigcallHub needs to broadcast frames to monitoring clients
        Self {
            dispatcher: RwLock::new(dispatcher),
            external_tools: ExternalToolManager::new(),
            turns: TurnRuntime::new(),
            sigcalls: SigcallHub::new(broadcast_tx),
            needs: NeedKernel::new(),
            tasks: TaskKernel::new(),
            rooms: RoomKernel::new(),
            tick: std::sync::OnceLock::new(),
            workspace,
            store: std::sync::OnceLock::new(),
            activity_seq: AtomicU64::new(0),
            activity_last_ms: AtomicI64::new(now_ms()),
            frames: std::sync::OnceLock::new(),
        }
    }

// =============================================================================
// ACCESSORS
// =============================================================================
//
// WHY getters: Encapsulate kernel subsystems rather than exposing fields.
// This allows future internal restructuring without breaking callers.

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Set the history store (called after kernel init by the runtime).
    ///
    /// WHY deferred: The Store requires a database path that may depend on
    /// kernel workspace resolution, so it's initialized after the kernel.
    pub fn set_store(&self, store: Arc<Store>) {
        let _ = self.store.set(store);
    }

    pub fn store(&self) -> Option<Arc<Store>> {
        self.store.get().cloned()
    }

    /// Bump activity timestamp and sequence.
    ///
    /// WHY: Used by syscalls to signal that work is happening, which informs
    /// idle detection for shutdown or sleep modes.
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

    /// Set the frame store and propagate it to subsystems.
    ///
    /// WHY propagation: The dispatcher and sigcall hub both emit frames that
    /// must be persisted. Setting the frame store centrally ensures consistency.
    pub async fn set_frames(&self, frames: Arc<FrameStore>) {
        if self.frames.set(frames.clone()).is_ok() {
            self.sigcalls.set_frames(frames.clone());
            let mut d = self.dispatcher_mut().await;
            d.set_frames(frames);
        }
    }

    pub fn frames(&self) -> Option<Arc<FrameStore>> {
        self.frames.get().cloned()
    }

    pub fn external_tools(&self) -> &ExternalToolManager {
        &self.external_tools
    }

    pub fn turns(&self) -> &TurnRuntime {
        &self.turns
    }

    pub fn sigcalls(&self) -> &SigcallHub {
        &self.sigcalls
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

    /// Start the kernel tick clock (called during kernel init).
    ///
    /// WHY: The tick clock drives time-based operations like mind wake cycles.
    /// The interval is configurable via KERNEL_TICK_MS environment variable.
    pub fn start_tick(&self) {
        let interval_ms = std::env::var("KERNEL_TICK_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(1000); // WHY 1000ms default: Balances responsiveness with CPU usage

        if self.tick.get().is_some() {
            return; // WHY: Idempotent; multiple calls are safe
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

// =============================================================================
// HELPERS
// =============================================================================

/// Get current time in milliseconds since UNIX epoch.
///
/// WHY: Activity tracking uses millisecond precision for idle detection.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
