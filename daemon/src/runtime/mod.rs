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
pub mod chaos;
mod collective;
mod config;
#[cfg(unix)]
mod frames_uds;
mod hand;
mod head;
mod kernel;
pub(crate) mod llm_harness;
pub mod mind;
pub mod parser;
pub mod preflight;
mod process_state;
pub mod room;
mod session_locks;
mod snapshot;
mod system_bundle;
mod system_bundler;
mod tars;
mod tool_logging;
pub mod trait_catalog;
mod workspace_config;

pub use app_config::{
    AppConfig, WorkspacePaths, atomic_write_file_0600, config_dir, default_config_path,
    default_frames_db_path, read_optional_file, workspace_config_from_root,
    workspace_dir_from_root, workspace_head_memory, workspace_name_from_root,
    workspace_transcripts_dir,
};

pub use bundle_layers::{build_environment_layer, build_network_layer};
pub use collective::{bump_reboot_epoch, reboot_epoch, rebooted_since};
pub use config::{Config, client_for_actor};
#[cfg(unix)]
pub use frames_uds::serve_frames_uds;
pub use hand::{
    HandBundleBuilder, HandBundleConfig, HandConfig, HandResult, HandService, execute_hand_loop,
};
pub use head::{HeadBundleBuilder, HeadBundleConfig, HeadConfig, HeadService};
pub use kernel::Kernel;

// Re-export from room module (canonical location)
pub use room::{
    AgentRoundResult, Room, RoomAgent, RoomConfig, RoomRunner, RoomType, TranscriptEntry,
};

pub use mind::{MindLoop, MindLoopBundleBuilder, MindLoopBundleConfig, MindLoopConfig};
pub use parser::{Block, extract_plain_text, parse_fenced_blocks, parse_quoted};
pub use preflight::run_preflight;
pub use process_state::{effective_bind_addr, set_effective_bind_addr};
pub use session_locks::{SessionWriteGuard, SessionWriteLocks};
pub use snapshot::{RuntimeSnapshot, SnapshotManager};
pub use system_bundle::{SystemBundle, SystemSlot};
pub use system_bundler::SystemBundler;
pub use tars::TarsDials;
pub use tool_logging::summarize_tool_args;
pub use workspace_config::WorkspaceConfigToml;
