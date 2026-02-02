// Runtime services for task execution and AI coordination.
//
// The runtime provides a multi-service architecture inspired by distributed cognition:
// - Mind: Strategic planner that creates needs based on observation ("why to do it")
// - Head: Tactical decision maker that converts needs to tasks ("what to do")
// - Hand: Operational executor that performs work using tools ("how to do it")
// - Need: kernel-owned queue (need:* syscalls)
// - Task: kernel-owned queue (task:* syscalls)
// - Exec: Tool dispatcher that routes tool calls to implementations
//
// Flow: Mind creates Need -> Head leases Need -> Head enqueues Task -> Hand leases Task
//
// Core services communicate via kernel syscalls/streams; persistence is via store.db + logs.db.

pub mod app_config;
mod bundle_layers;
mod bus;
mod collective;
mod conclave;
mod config;
mod hand_allocator;
mod hand_bundle;
mod hand_config;
mod hand_service;
mod head_bundle;
mod head_config;
mod head_service;
// idle_monitor_service removed (idle tracking lives in MindService + kernel activity)
mod kernel;
mod llm_harness;
mod mind_bundle;
mod mind_config;
mod mind_service;
mod models_config;
// need_service removed (kernel need:* syscalls)
pub mod parser;
mod plugins;
mod proc_service;
// recall_flush_service removed (not needed currently)
mod room;
// room_coordinator removed (rooms are kernel-owned via room:* syscalls)
mod session_locks;
mod snapshot;
// stat_service removed (not needed currently)
mod tars;
// task_service removed (kernel task:* syscalls)
mod tool_logging;

pub use app_config::{
    AppConfig, WorkspacePaths, atomic_write_file_0600, config_dir, default_config_path,
    default_models_path, read_optional_file, workspace_config_from_root, workspace_dir_from_root,
    workspace_head_memory, workspace_mind_memory, workspace_mind_self, workspace_name_from_root,
    workspace_plugins_config, workspace_transcripts_dir,
};
pub use bundle_layers::{build_environment_layer, build_network_layer};
pub use bus::RuntimeBus;
pub use collective::{bump_reboot_epoch, reboot_epoch, rebooted_since};
pub use conclave::Conclave;
pub use config::Config;
pub use hand_allocator::HandAllocator;
pub use hand_bundle::{AutistMode, HandBundleBuilder, HandBundleConfig};
pub use hand_config::HandConfig;
pub use hand_service::HandService;
pub use head_bundle::{GenerationMode, HeadBundleBuilder, HeadBundleConfig};
pub use head_config::HeadConfig;
pub use head_service::HeadService;
pub use kernel::Kernel;
pub use mind_bundle::{FeverMode, MindBundleBuilder, MindBundleConfig, RoomType, WakeMode};
pub use mind_config::MindConfig;
pub use mind_service::MindService;
pub use models_config::{ModelDef, ModelsConfig};
pub use parser::{Block, extract_plain_text, parse_fenced_blocks, parse_quoted};
pub use plugins::PluginManager;
pub use proc_service::{ProcHandle, ProcKind, ProcService};
pub use room::{
    LtmProposal, MindPersona, NeedProposal, Room, RoomDecision, RoomKind, RoomMessage, RoomStatus,
    WantProposal,
};
pub use session_locks::{SessionWriteGuard, SessionWriteLocks};
pub use snapshot::{RuntimeSnapshot, SnapshotManager};
pub use tars::TarsDials;
pub use tool_logging::summarize_tool_args;
