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
mod bus;
mod config;
mod task_service;
mod hand_allocator;
mod parser;
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
mod stat_service;
mod recall_flush_service;
mod idle_monitor_service;
mod room;
mod conclave;
mod plugins;

pub use app_config::{
    AppConfig, config_dir, default_config_path, default_models_path,
    data_dir, sandbox_dir, sandbox_workspace, sandbox_db, sandbox_recall_db,
    sandbox_env, create_sandbox_env, load_sandbox_env,
    create_sandbox_mind_metadata,
    sandbox_mind_dir, sandbox_mind_memory_md, sandbox_mind_self_md,
    sandbox_head_dir, sandbox_head_memory_md,
    sandbox_dir_from_workspace_root, sandbox_name_from_workspace_root,
    sandbox_mind_memory_from_workspace_root, sandbox_mind_self_from_workspace_root,
    sandbox_head_memory_from_workspace_root,
    read_optional_file, atomic_write_file_0600,
};
pub use bus::RuntimeBus;
pub use config::Config;
pub use models_config::{ModelDef, ModelsConfig};
pub use task_service::TaskService;
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
pub use need_service::{NeedService, Need, HeadInfo, HeadState};
pub use stat_service::StatService;
pub use recall_flush_service::RecallFlushService;
pub use idle_monitor_service::IdleMonitorService;
pub use room::{Room, RoomKind, RoomStatus, RoomMessage, RoomDecision, MindPersona, NeedProposal, WantProposal, LtmProposal};
pub use conclave::Conclave;
pub use plugins::PluginManager;
