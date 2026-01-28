mod bash;
mod cd;
mod diff;
mod find;
mod logs;
mod monk;
mod patch;
mod post;
mod read;
mod write;
mod dispatcher;
mod agent;
pub mod validator;

use std::path::PathBuf;

pub use bash::BashTool;
pub use cd::CdTool;
pub use diff::DiffTool;
pub use find::FindTool;
pub use logs::LogsTool;
pub use monk::MonkTool;
pub use patch::PatchTool;
pub use post::PostTool;
pub use read::ReadTool;
pub use write::WriteTool;
pub use dispatcher::Dispatcher;
pub use agent::ToolAgent;
pub use validator::{Validator, ValidationContext, ValidationResult, ValidatorChain, AllowAll, AllowTools, DenyTools};

pub struct ExecutionContext {
    pub cwd: PathBuf,
    pub sender: String,
    pub channel: String,
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String;
}
