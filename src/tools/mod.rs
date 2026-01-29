mod bash;
mod cd;
mod channel;
mod diff;
mod edit;
mod find;
mod garden;
mod monk;
mod petition;
mod pray;
mod read;
mod self_tool;
mod workspace;
mod write;
mod dispatcher;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use crate::bus::Hub;
use crate::history::Store;
use crate::monk::SharedRegistry;

pub use bash::BashTool;
pub use cd::CdTool;
pub use channel::ChannelTool;
pub use diff::DiffTool;
pub use edit::EditTool;
pub use find::FindTool;
pub use garden::GardenTool;
pub use monk::MonkTool;
pub use petition::PetitionTool;
pub use pray::PrayTool;
pub use read::ReadTool;
pub use self_tool::SelfTool;
pub use workspace::WorkspaceTool;
pub use write::WriteTool;
pub use dispatcher::Dispatcher;

/// Shared working directory that can be modified by tools (e.g., cd)
pub type SharedCwd = Arc<Mutex<PathBuf>>;

#[derive(Clone)]
pub struct ExecutionContext {
    pub cwd: SharedCwd,
    pub sender: String,
    pub channel: String,
    pub store: Arc<Store>,
    pub registry: SharedRegistry,
    pub hub: Arc<RwLock<Hub>>,
}

#[cfg(test)]
pub mod test_utils {
    use super::*;
    use crate::monk::new_registry;

    pub fn test_context(cwd: impl Into<PathBuf>) -> ExecutionContext {
        ExecutionContext {
            cwd: Arc::new(Mutex::new(cwd.into())),
            sender: "test-user".to_string(),
            channel: "#test".to_string(),
            store: Arc::new(Store::open(":memory:").unwrap()),
            registry: new_registry(),
            hub: Arc::new(RwLock::new(Hub::new())),
        }
    }
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String;
}
