// Runtime services for task execution and AI coordination.
//
// The runtime provides a multi-service architecture inspired by distributed cognition:
// - Mind: Strategic planner that creates needs based on observation ("why to do it")
// - Head: Tactical decision maker that converts needs to goals ("what to do")
// - Hand: Operational executor that performs work using tools ("how to do it")
// - Need: Coordinator that dispatches needs from Mind to Head pool
// - Goal: Coordinator that dispatches goals from Head to Hand pool
// - Exec: Tool dispatcher that routes tool calls to implementations
//
// Flow: Mind creates Need -> NeedService -> Head creates Goal -> GoalService -> Hand
//
// Services communicate via the bus and persist state through SQLite.

mod app_config;
mod bus;
mod config;
mod exec;
mod goal_service;
mod hand_allocator;
mod parser;
mod hand_bundle;
mod hand_config;
mod hand_parser;
mod hand_service;
mod llm_harness;
mod head_bundle;
mod head_config;
mod head_parser;
mod head_service;
mod mind_bundle;
mod mind_config;
mod mind_parser;
mod mind_service;
mod models_config;
mod need_service;
mod room;
mod conclave;

pub use app_config::AppConfig;
pub use bus::RuntimeBus;
pub use config::Config;
pub use models_config::{ModelDef, ModelsConfig};
pub use goal_service::GoalService;
pub use parser::{parse_fenced_blocks, parse_quoted, extract_plain_text, Block};
pub use exec::{ExecService, ExecServiceConfig};
pub use hand_allocator::HandAllocator;
pub use hand_bundle::{HandBundleBuilder, HandBundleConfig};
pub use hand_config::HandConfig;
pub use hand_parser::{parse_hand_response, ExecAction, ParsedHandResponse, ResultAction};
pub use hand_service::HandService;
pub use head_bundle::{HeadBundleBuilder, HeadBundleConfig};
pub use head_config::HeadConfig;
pub use head_parser::{parse_head_response, ChatAction, MailAction, GoalAction, ParsedHeadResponse};
pub use head_service::HeadService;
pub use mind_bundle::{MindBundleBuilder, MindBundleConfig};
pub use mind_config::MindConfig;
pub use mind_parser::{parse_mind_response, LtmAction, ParsedMindResponse};
pub use mind_service::MindService;
pub use need_service::{NeedService, Need, HeadInfo, HeadState};
pub use room::{Room, RoomKind, RoomStatus, RoomMessage, RoomDecision, MindPersona, NeedProposal, WantProposal, LtmProposal};
pub use conclave::Conclave;
