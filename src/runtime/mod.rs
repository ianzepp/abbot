// Runtime services for task execution and AI coordination.
//
// The runtime provides a multi-service architecture inspired by distributed cognition:
// - Head: AI decision maker that processes input and creates tasks ("what to do")
// - Mind: Background reflector that maintains long-term memory ("why to do it")
// - Hand: Task executor that performs actual work using tools
// - Goal: Task coordinator that manages the task lifecycle
// - Exec: Tool dispatcher that routes tool calls to implementations
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
