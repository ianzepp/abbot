mod bash;
mod cd;
mod channel;
mod diff;
mod find;
mod monk;
mod patch;
mod read;
mod self_tool;
mod workspace;
mod write;
mod dispatcher;

use std::path::PathBuf;
use std::sync::Arc;
use crate::history::Store;
use crate::monk::SharedRegistry;

pub use bash::BashTool;
pub use cd::CdTool;
pub use channel::ChannelTool;
pub use diff::DiffTool;
pub use find::FindTool;
pub use monk::MonkTool;
pub use patch::PatchTool;
pub use read::ReadTool;
pub use self_tool::SelfTool;
pub use workspace::WorkspaceTool;
pub use write::WriteTool;
pub use dispatcher::Dispatcher;

#[derive(Clone)]
pub struct ExecutionContext {
    pub cwd: PathBuf,
    pub sender: String,
    pub channel: String,
    pub store: Arc<Store>,
    pub registry: SharedRegistry,
}

#[cfg(test)]
pub mod test_utils {
    use super::*;
    use crate::monk::new_registry;

    pub fn test_context(cwd: impl Into<PathBuf>) -> ExecutionContext {
        ExecutionContext {
            cwd: cwd.into(),
            sender: "test-user".to_string(),
            channel: "#test".to_string(),
            store: Arc::new(Store::open(":memory:").unwrap()),
            registry: new_registry(),
        }
    }
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String;
}
