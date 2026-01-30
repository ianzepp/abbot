// Runtime services for task execution and AI coordination.
//
// The runtime provides a multi-service architecture inspired by human organization:
// - Head: AI decision maker that processes input and creates tasks
// - Heart: Background monitor that periodically checks system state
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
mod head_bundle;
mod head_config;
mod head_parser;
mod head_service;
mod heart_bundle;
mod heart_config;
mod heart_parser;
mod heart_service;
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
pub use heart_bundle::{HeartBundleBuilder, HeartBundleConfig};
pub use heart_config::HeartConfig;
pub use heart_parser::{parse_heart_response, LtmAction, ParsedHeartResponse};
pub use heart_service::HeartService;
