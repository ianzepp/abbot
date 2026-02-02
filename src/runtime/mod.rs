// Runtime services for task execution and AI coordination.
//
// The runtime provides a multi-service architecture inspired by distributed cognition:
// - Mind: Strategic planner that creates needs based on observation ("why to do it")
// - Head: Tactical decision maker that converts needs to tasks ("what to do")
// - Hand: Operational executor that performs work using tools ("how to do it")
// - Need: Coordinator that dispatches needs from Mind to Head pool
// - Task: Coordinator that dispatches tasks from Head to Hand pool
// - Exec: Tool dispatcher that routes tool calls to implementations
//
// Flow: Mind creates Need -> NeedService -> Head creates Task -> TaskService -> Hand
//
// Services communicate via the bus and persist state through SQLite.

pub mod app_config;
mod collective;
mod bus;
mod config;
mod kernel;
mod snapshot;
mod tool_logging;
mod task_service;
mod hand_allocator;
pub mod parser;
mod hand_bundle;
mod hand_config;
mod hand_service;
mod llm_harness;
mod head_bundle;
mod head_config;
mod head_service;
mod mind_bundle;
mod mind_config;
mod mind_service;
mod models_config;
mod need_service;
mod proc_service;
mod stat_service;
mod recall_flush_service;
mod idle_monitor_service;
mod room;
mod room_coordinator;
mod conclave;
mod plugins;
mod bundle_layers;
mod tars;
mod session_locks;

pub use app_config::{
    AppConfig, WorkspacePaths, config_dir, default_config_path, default_models_path,
    workspace_dir_from_root, workspace_mind_memory, workspace_mind_self,
    workspace_head_memory, workspace_config_from_root, workspace_plugins_config,
    workspace_name_from_root, workspace_transcripts_dir,
    read_optional_file, atomic_write_file_0600,
};
pub use collective::{reboot_epoch, bump_reboot_epoch, rebooted_since};
pub use bus::RuntimeBus;
pub use config::Config;
pub use snapshot::{RuntimeSnapshot, SnapshotManager};
pub use tool_logging::summarize_tool_args;
pub use models_config::{ModelDef, ModelsConfig};
pub use task_service::{TaskService, TaskServiceQuery, Task, HandInfo, HandState};
pub use parser::{parse_fenced_blocks, parse_quoted, extract_plain_text, Block};
pub use hand_allocator::HandAllocator;
pub use hand_bundle::{HandBundleBuilder, HandBundleConfig, AutistMode};
pub use hand_config::HandConfig;
pub use hand_service::HandService;
pub use head_bundle::{HeadBundleBuilder, HeadBundleConfig, GenerationMode};
pub use head_config::HeadConfig;
pub use head_service::HeadService;
pub use mind_bundle::{MindBundleBuilder, MindBundleConfig, WakeMode, FeverMode, RoomType};
pub use mind_config::MindConfig;
pub use mind_service::MindService;
pub use need_service::{NeedService, Need, HeadInfo as NeedHeadInfo, HeadState as NeedHeadState};
pub use proc_service::{ProcHandle, ProcKind, ProcService};
pub use stat_service::StatService;
pub use recall_flush_service::RecallFlushService;
pub use idle_monitor_service::IdleMonitorService;
pub use room::{Room, RoomKind, RoomStatus, RoomMessage, RoomDecision, MindPersona, NeedProposal, WantProposal, LtmProposal};
pub use room_coordinator::{RoomCoordinator, RoomBus, RoomResult, MindResponse, Proposal, ProposalKind, Vote};
pub use conclave::Conclave;
pub use plugins::PluginManager;
pub use bundle_layers::{build_environment_layer, build_network_layer};
pub use tars::TarsDials;
pub use session_locks::{SessionWriteLocks, SessionWriteGuard};
pub use kernel::Kernel;
