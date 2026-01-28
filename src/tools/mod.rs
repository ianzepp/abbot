mod bash;
mod diff;
mod edit;
mod find;
mod read;
mod dispatcher;
mod agent;

pub use bash::BashTool;
pub use diff::DiffTool;
pub use edit::EditTool;
pub use find::FindTool;
pub use read::ReadTool;
pub use dispatcher::Dispatcher;
pub use agent::ToolAgent;

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn execute(&self, args: &str) -> String;
}
