//! Chat handler runtime abstraction for testability.

use std::path::PathBuf;

use async_trait::async_trait;
use uuid::Uuid;

use crate::kernel::{Frame, KernelReceiver};
use crate::runtime::Kernel;

#[async_trait]
pub trait ChatRuntime: Send + Sync {
    fn workspace(&self) -> PathBuf;

    async fn open_stream(
        &self,
        room: &str,
        thread_id: Uuid,
    ) -> Result<tokio::sync::mpsc::Receiver<Frame>, String>;

    async fn dispatch(&self, req: Frame, workspace: PathBuf) -> Result<KernelReceiver, String>;
}

pub struct KernelChatRuntime;

impl Default for KernelChatRuntime {
    fn default() -> Self {
        Self
    }
}

impl KernelChatRuntime {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ChatRuntime for KernelChatRuntime {
    fn workspace(&self) -> PathBuf {
        Kernel::get()
            .map(|k| k.workspace().to_path_buf())
            .unwrap_or_default()
    }

    async fn open_stream(
        &self,
        room: &str,
        thread_id: Uuid,
    ) -> Result<tokio::sync::mpsc::Receiver<Frame>, String> {
        let Some(k) = Kernel::get() else {
            return Err("Kernel not initialized".to_string());
        };
        Ok(k.sigcalls().open(room, thread_id).await)
    }

    async fn dispatch(&self, req: Frame, workspace: PathBuf) -> Result<KernelReceiver, String> {
        let Some(k) = Kernel::get() else {
            return Err("Kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        Ok(dispatcher.dispatch(req, workspace, tokio_util::sync::CancellationToken::new()))
    }
}
