use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};

use super::error::KernelError;
use super::frame::{Frame, FrameOp};
use super::syscall::{Syscall, SyscallContext};

pub struct KernelDispatcher {
    handlers: HashMap<String, Arc<dyn Syscall>>,
}

impl KernelDispatcher {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    pub fn register(&mut self, syscall: Arc<dyn Syscall>) {
        let name = syscall.name().to_string();
        self.handlers.insert(name, syscall);
    }

    pub fn has(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    pub fn list(&self) -> Vec<&str> {
        self.handlers.keys().map(|s| s.as_str()).collect()
    }

    #[instrument(skip(self, req, cancel), fields(call_id = %req.id, name = ?req.name))]
    pub fn dispatch(
        &self,
        req: Frame,
        cwd: PathBuf,
        cancel: CancellationToken,
    ) -> mpsc::Receiver<Frame> {
        let (tx, rx) = mpsc::channel(32);

        if req.op != FrameOp::Req {
            let err = KernelError::invalid_args("dispatch expects a Req frame");
            let _ = tx.try_send(Frame::error(req.id, err.to_value()));
            return rx;
        }

        let name = match &req.name {
            Some(n) => n.clone(),
            None => {
                let err = KernelError::invalid_args("Req frame missing 'name' field");
                let _ = tx.try_send(Frame::error(req.id, err.to_value()));
                return rx;
            }
        };

        let handler = match self.handlers.get(&name) {
            Some(h) => Arc::clone(h),
            None => {
                let err = KernelError::not_implemented(&name);
                let _ = tx.try_send(Frame::error(req.id, err.to_value()));
                return rx;
            }
        };

        let data = req.data.clone().unwrap_or(serde_json::Value::Null);
        let call_id = req.id;
        let actor = req.actor.clone();
        let deadline_ms = req.deadline_ms;

        info!("kernel req received");

        tokio::spawn(async move {
            let start = Instant::now();

            let ctx = SyscallContext::new(call_id, cwd, cancel.clone())
                .with_actor(actor)
                .with_deadline(deadline_ms);

            let timeout = deadline_ms.map(Duration::from_millis);

            let result = if let Some(timeout) = timeout {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        Err(KernelError::cancelled("operation cancelled"))
                    }
                    _ = tokio::time::sleep(timeout) => {
                        Err(KernelError::timeout(format!("syscall exceeded {}ms deadline", timeout.as_millis())))
                    }
                    result = handler.execute(&ctx, data, tx.clone()) => result
                }
            } else {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        Err(KernelError::cancelled("operation cancelled"))
                    }
                    result = handler.execute(&ctx, data, tx.clone()) => result
                }
            };

            let elapsed = start.elapsed().as_millis();

            match result {
                Ok(()) => {
                    info!(duration_ms = elapsed, "kernel ok emitted");
                }
                Err(e) => {
                    warn!(duration_ms = elapsed, code = %e.code, "kernel error emitted");
                    let _ = tx.send(Frame::error(call_id, e.to_value())).await;
                }
            }
        });

        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct EchoSyscall;

    #[async_trait::async_trait]
    impl Syscall for EchoSyscall {
        fn name(&self) -> &'static str {
            "test:echo"
        }

        async fn execute(
            &self,
            ctx: &SyscallContext,
            data: serde_json::Value,
            tx: mpsc::Sender<Frame>,
        ) -> Result<(), KernelError> {
            ctx.check_cancelled()?;
            tx.send(Frame::ok(ctx.call_id, data)).await.ok();
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_dispatcher_register_and_dispatch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();

        let mut dispatcher = KernelDispatcher::new();
        dispatcher.register(Arc::new(EchoSyscall));

        assert!(dispatcher.has("test:echo"));
        assert!(!dispatcher.has("test:missing"));

        let req = Frame::req("test:echo", json!({"msg": "hello"}));
        let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

        let response = rx.recv().await.expect("should receive response");
        assert_eq!(response.op, FrameOp::Ok);
        assert_eq!(response.parent_id, Some(req.id));
        assert_eq!(response.data.unwrap()["msg"], "hello");
    }

    #[tokio::test]
    async fn test_dispatcher_unknown_syscall() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();

        let dispatcher = KernelDispatcher::new();

        let req = Frame::req("unknown:syscall", json!({}));
        let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

        let response = rx.recv().await.expect("should receive error");
        assert_eq!(response.op, FrameOp::Error);
        assert_eq!(response.parent_id, Some(req.id));
        assert!(response.data.unwrap()["code"].as_str().unwrap().contains("NOT_IMPLEMENTED"));
    }

    #[tokio::test]
    async fn test_dispatcher_invalid_frame() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();

        let dispatcher = KernelDispatcher::new();

        let bad_frame = Frame::ok(uuid::Uuid::new_v4(), json!({}));
        let mut rx = dispatcher.dispatch(bad_frame.clone(), workspace, CancellationToken::new());

        let response = rx.recv().await.expect("should receive error");
        assert_eq!(response.op, FrameOp::Error);
    }
}
