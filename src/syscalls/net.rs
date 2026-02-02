use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::hal::{HalHttpRequest, HalNet, HalNetError, HostHalNet};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

#[derive(Debug, Deserialize)]
struct NetFetchArgs {
    url: String,
    #[serde(default = "default_method")]
    method: String,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    body_base64: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_body_bytes: Option<usize>,
}

fn default_method() -> String {
    "GET".to_string()
}

pub struct NetFetch {
    net: Arc<dyn HalNet>,
}

impl NetFetch {
    pub fn new() -> Self {
        Self {
            net: Arc::new(HostHalNet),
        }
    }

    pub fn with_net(net: Arc<dyn HalNet>) -> Self {
        Self { net }
    }
}

impl Default for NetFetch {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for NetFetch {
    fn name(&self) -> &'static str {
        "net:fetch"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: NetFetchArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.url.is_empty() {
            return Err(KernelError::invalid_args("'url' is required"));
        }

        if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
            return Err(KernelError::invalid_args(
                "url must start with http:// or https://",
            ));
        }

        let body = if let Some(b64) = args.body_base64 {
            Some(
                base64::engine::general_purpose::STANDARD
                    .decode(&b64)
                    .map_err(|e| KernelError::invalid_args(format!("invalid base64 body: {e}")))?,
            )
        } else {
            args.body.map(|s| s.into_bytes())
        };

        let timeout = args
            .timeout_ms
            .or(ctx.deadline_ms)
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(30));

        let max_body_bytes = args.max_body_bytes.unwrap_or(10 * 1024 * 1024);

        ctx.check_cancelled()?;

        let req = HalHttpRequest {
            method: args.method.to_uppercase(),
            url: args.url.clone(),
            headers: args.headers.unwrap_or_default(),
            body,
            timeout,
            max_body_bytes,
        };

        let response = self.net.http_request(req).await.map_err(|e| match e {
            HalNetError::InvalidArgs(msg) => KernelError::invalid_args(msg),
            HalNetError::Timeout { timeout } => {
                KernelError::timeout(format!("request timed out after {:?}", timeout))
            }
            HalNetError::Connect(msg) => KernelError::io(format!("connection failed: {msg}")),
            HalNetError::Http(msg) => KernelError::io(format!("http error: {msg}")),
        })?;

        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(&response.body);

        tx.send(Frame::ok(
            ctx.call_id,
            json!({
                "status": response.status,
                "headers": response.headers,
                "body": body_text,
                "body_base64": body_base64,
                "truncated": response.truncated,
                "body_size": response.body_size,
            }),
        ))
        .await
        .ok();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    fn make_ctx(cwd: &std::path::Path) -> SyscallContext {
        SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
    }

    #[tokio::test]
    async fn test_net_fetch_invalid_url() {
        let tmp = TempDir::new().unwrap();
        let syscall = NetFetch::new();
        let ctx = make_ctx(tmp.path());
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "url": "not-a-url" }), tx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARGS");
    }

    #[tokio::test]
    async fn test_net_fetch_empty_url() {
        let tmp = TempDir::new().unwrap();
        let syscall = NetFetch::new();
        let ctx = make_ctx(tmp.path());
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall.execute(&ctx, json!({ "url": "" }), tx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARGS");
    }

    #[test]
    fn test_default_method() {
        assert_eq!(default_method(), "GET");
    }
}
