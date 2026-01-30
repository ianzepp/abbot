// Tool implementations for file and shell operations.
//
// Tools are the only way hands interact with the external system. Each tool
// implements the Tool trait and is registered with the Dispatcher. The ExecService
// routes tool calls from hands to the appropriate implementation. All tools
// respect the shared working directory (SharedCwd) so cd affects subsequent
// operations in the same task.
//
// Security note: Tools execute with the privileges of the abbot process. The
// bash tool in particular runs arbitrary shell commands and should only be
// used in trusted environments.

mod bash;
mod cd;
mod diff;
mod edit;
mod find;
mod patch;
mod read;
mod write;
mod dispatcher;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::bus::Scope;

pub use bash::BashTool;
pub use cd::CdTool;
pub use diff::DiffTool;
pub use edit::EditTool;
pub use find::FindTool;
pub use patch::PatchTool;
pub use read::ReadTool;
pub use write::WriteTool;
pub use dispatcher::Dispatcher;

/// Shared working directory that can be modified by tools (e.g., cd)
pub type SharedCwd = Arc<Mutex<PathBuf>>;

#[derive(Clone)]
pub struct ExecutionContext {
    pub cwd: SharedCwd,
    pub sender: String,
    pub scope: Scope,
}

#[cfg(test)]
pub mod test_utils {
    use super::*;

    pub fn test_context(cwd: impl Into<PathBuf>) -> ExecutionContext {
        ExecutionContext {
            cwd: Arc::new(Mutex::new(cwd.into())),
            sender: "test-user".to_string(),
            scope: Scope::from("#test"),
        }
    }
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String;
}
