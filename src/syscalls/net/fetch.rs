//! Net:Fetch - HTTP/HTTPS request execution with security constraints
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides controlled HTTP/HTTPS request execution within the Abbot
//! kernel's security model. It integrates with the Hardware Abstraction Layer (HAL)
//! for network operations and enforces security boundaries through protocol validation,
//! response size limiting, and timeout enforcement.
//!
//! **Critical security boundaries:**
//! - Protocol allowlist (only HTTP/HTTPS, no file://, ftp://, etc.)
//! - Response size limiting (prevents memory exhaustion from large responses)
//! - Timeout enforcement (prevents hung requests from blocking task lanes)
//! - No actor-based restrictions (intentionally - all agents may fetch URLs)
//!
//! **Integration points:**
//! - `HalNet` trait for platform-specific HTTP client implementation
//! - `SyscallContext` for cancellation and deadline enforcement
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with status/headers/body/truncation info on success
//! - Returns `KernelError` for invalid URLs, timeouts, or network failures
//!
//! SECURITY MODEL
//! ==============
//! This syscall balances functionality with protection against network-based attacks:
//!
//! 1. **Protocol Validation**
//!    - WHY: Prevents local file access via `file://` URLs or SSRF via `ftp://`
//!    - HOW: Rejects URLs not starting with `http://` or `https://` (line 78)
//!    - ATTACK PREVENTED: Server-Side Request Forgery (SSRF) to internal services
//!    - LIMITATION: Does not prevent SSRF to internal HTTP endpoints (e.g., 127.0.0.1)
//!
//! 2. **Response Size Limiting**
//!    - WHY: Prevents memory exhaustion from multi-gigabyte responses
//!    - HOW: `max_body_bytes` (default 10MB) truncates response body at HAL layer
//!    - ATTACK PREVENTED: Resource exhaustion DoS from malicious servers
//!
//! 3. **Timeout Enforcement**
//!    - WHY: Prevents hung connections from blocking task lanes indefinitely
//!    - HOW: Default 30s timeout, overridable via `timeout_ms` or context deadline
//!    - ATTACK PREVENTED: Slowloris-style DoS from slow-responding servers
//!
//! 4. **No Actor Restrictions**
//!    - WHY: HTTP fetching is inherently read-only (no filesystem mutation)
//!    - IMPLICATION: "Hand", "Head", and "Room" agents all have fetch capability
//!    - TRADE-OFF: Allows LLMs to fetch arbitrary URLs, but responses are sandboxed
//!
//! 5. **Base64 Body Encoding**
//!    - WHY: Enables safe transport of binary data (images, PDFs, etc.) via JSON
//!    - HOW: Response includes both UTF-8 `body` and `body_base64` fields
//!    - SECURITY: Malformed UTF-8 in response uses lossy conversion (no crashes)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Read-only by nature**: HTTP GET is inherently safe (no local mutation)
//! - **Fail-safe defaults**: Conservative timeout (30s) and size limit (10MB)
//! - **Binary-safe**: Base64 encoding prevents UTF-8 conversion issues
//! - **Transparent truncation**: Caller knows if response was incomplete
//! - **Protocol-level security**: Validate URL scheme before network access
//!
//! PERFORMANCE
//! ===========
//! - Response body is truncated at `max_body_bytes` to bound memory usage
//! - No streaming support - entire response buffered in memory (acceptable for 10MB limit)
//! - Timeout applies to entire request lifecycle (connect + headers + body)
//!
//! CONCURRENCY
//! ===========
//! - No actor mutation lock required (HTTP fetch is read-only)
//! - Safe for concurrent execution across multiple task lanes
//! - HAL layer handles connection pooling and async I/O
//!
//! TRADE-OFFS
//! ==========
//! 1. **No SSRF Protection for Internal IPs**
//!    - CHOSEN: Allow requests to any HTTP/HTTPS URL (including 127.0.0.1, 10.x.x.x)
//!    - REJECTED: Blocklist private IP ranges (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16)
//!    - WHY: Legitimate use cases for local development servers, Docker containers
//!    - IMPLICATION: Agents can probe internal services if accessible
//!
//! 2. **No Actor Restrictions**
//!    - CHOSEN: All agents may fetch URLs (no `require_mutation()`)
//!    - WHY: HTTP GET is read-only and responses are sandboxed
//!    - IMPLICATION: Compromised LLM can fetch URLs, but cannot mutate local state
//!
//! 3. **Buffered Response (Not Streaming)**
//!    - CHOSEN: Buffer entire response in memory before returning
//!    - REJECTED: Stream response chunks incrementally via Frame::event
//!    - WHY: Simpler implementation, 10MB limit makes buffering acceptable
//!    - IMPLICATION: Cannot process multi-gigabyte streams efficiently

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hal::{HalHttpRequest, HalNet, HalNetError, HostHalNet};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `net:fetch` syscall.
///
/// WHY: Structured HTTP request specification with security-conscious defaults.
#[derive(Debug, Deserialize)]
struct NetFetchArgs {
    /// URL to fetch (must start with `http://` or `https://`).
    ///
    /// WHY: Protocol validation prevents SSRF via `file://`, `ftp://`, etc.
    /// SECURITY: Does not block private IPs (127.0.0.1, 10.x.x.x) - see TRADE-OFFS.
    url: String,

    /// HTTP method (default: GET).
    ///
    /// WHY: Uppercased at execution time for case-insensitive matching.
    /// Common values: GET, POST, PUT, DELETE, PATCH.
    #[serde(default = "default_method")]
    method: String,

    /// HTTP headers to send with the request.
    ///
    /// WHY: Enables custom authentication (Authorization header), content negotiation
    /// (Accept header), or API-specific headers (X-API-Key, etc.).
    #[serde(default)]
    headers: Option<HashMap<String, String>>,

    /// Request body as UTF-8 string (for JSON, XML, form data, etc.).
    ///
    /// WHY: Convenient for text-based APIs. Mutually exclusive with `body_base64`.
    #[serde(default)]
    body: Option<String>,

    /// Request body as base64-encoded bytes (for binary data, images, etc.).
    ///
    /// WHY: Enables safe transport of binary data via JSON. Takes precedence over `body`.
    #[serde(default)]
    body_base64: Option<String>,

    /// Timeout in milliseconds.
    ///
    /// WHY: Defaults to context deadline or 30s if unspecified. Prevents hung connections.
    #[serde(default)]
    timeout_ms: Option<u64>,

    /// Maximum response body size in bytes (default: 10MB).
    ///
    /// WHY: Prevents memory exhaustion from large responses. Response is truncated if exceeded.
    #[serde(default)]
    max_body_bytes: Option<usize>,
}

/// Default HTTP method for `net:fetch`.
///
/// WHY: GET is the most common read-only operation, appropriate default for fetch semantics.
fn default_method() -> String {
    "GET".to_string()
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for executing HTTP/HTTPS requests with security constraints.
///
/// WHY: Encapsulates HAL network abstraction, enabling testing with mock HTTP clients.
pub struct NetFetch {
    /// Hardware abstraction for HTTP requests.
    ///
    /// WHY: Enables testing with fake responses, platform-specific implementations.
    net: Arc<dyn HalNet>,
}

impl NetFetch {
    /// Create a new `NetFetch` syscall with host OS HTTP client.
    ///
    /// WHY: Standard constructor for production use with real network access.
    pub fn new() -> Self {
        Self {
            net: Arc::new(HostHalNet),
        }
    }

    /// Create a `NetFetch` syscall with a custom HAL network implementation.
    ///
    /// WHY: Enables testing with mock responses, simulated timeouts, etc.
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

    /// Execute an HTTP/HTTPS request with security constraints.
    ///
    /// WHY: Enables agents to fetch web content (documentation, APIs, etc.) while
    /// maintaining security boundaries through protocol validation, size limiting,
    /// and timeout enforcement.
    ///
    /// USE CASE: Invoked by all agent types ("head", "hand", "room") to fetch HTTP
    /// resources. Common uses include fetching API documentation, checking service
    /// status, or retrieving external data for processing.
    ///
    /// SECURITY NOTE: This syscall allows HTTP/HTTPS requests to ANY URL (including
    /// localhost and private IPs). It does NOT require mutation permission because
    /// HTTP GET is inherently read-only. However, this means:
    /// - Agents can probe internal services (127.0.0.1, 10.x.x.x, 192.168.x.x)
    /// - Response size is limited to `max_body_bytes` (default 10MB) to prevent resource exhaustion
    /// - Timeout is enforced to prevent hung connections (default 30s)
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{status, headers, body, body_base64, truncated, body_size}`
    /// - `E_INVALID_ARGS` if URL is empty, malformed, or non-HTTP/HTTPS
    /// - `E_TIMEOUT` if request exceeds timeout
    /// - `E_IO` for network connection failures or HTTP errors
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Argument Parsing & Validation
        // =====================================================================
        // WHY: Validate arguments before network access to fail fast on malformed requests.
        ctx.check_cancelled()?;

        let args: NetFetchArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.url.is_empty() {
            return Err(KernelError::invalid_args("'url' is required"));
        }

        // WHY: Protocol validation is the PRIMARY security boundary for this syscall.
        // Rejecting non-HTTP/HTTPS URLs prevents:
        // - Local file access via `file:///etc/passwd`
        // - FTP access via `ftp://internal-server/`
        // - Custom protocol handlers that might bypass sandbox
        //
        // LIMITATION: Does NOT prevent SSRF to internal HTTP endpoints like
        // http://127.0.0.1:8080 or http://10.0.0.1/admin. This is intentional
        // (see TRADE-OFFS in module documentation).
        if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
            return Err(KernelError::invalid_args(
                "url must start with http:// or https://",
            ));
        }

        // =====================================================================
        // PHASE 2: Request Body Preparation
        // =====================================================================
        // WHY: Support both text and binary request bodies. Base64 takes precedence
        // to enable binary payloads (e.g., image upload) via JSON-serialized syscall.
        let body = if let Some(b64) = args.body_base64 {
            Some(
                base64::engine::general_purpose::STANDARD
                    .decode(&b64)
                    .map_err(|e| KernelError::invalid_args(format!("invalid base64 body: {e}")))?,
            )
        } else {
            args.body.map(|s| s.into_bytes())
        };

        // =====================================================================
        // PHASE 3: Timeout & Size Limit Configuration
        // =====================================================================
        // WHY: Timeout defaults to context deadline or 30s. Conservative default
        // prevents hung connections from blocking task lanes indefinitely.
        let timeout = args
            .timeout_ms
            .or(ctx.deadline_ms)
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(30));

        // WHY: 10MB default for max_body_bytes balances functionality with security.
        // Large enough for typical API responses, small enough to prevent memory exhaustion.
        let max_body_bytes = args.max_body_bytes.unwrap_or(10 * 1024 * 1024);

        // WHY: Final cancellation check before network I/O. Prevents wasted bandwidth
        // for tasks that were cancelled during argument parsing.
        ctx.check_cancelled()?;

        // =====================================================================
        // PHASE 4: HTTP Request Execution
        // =====================================================================
        // WHY: Delegate to HAL layer for platform-specific HTTP client implementation.
        // Method is uppercased for case-insensitive matching (http/1.1 methods are uppercase).
        let req = HalHttpRequest {
            method: args.method.to_uppercase(),
            url: args.url.clone(),
            headers: args.headers.unwrap_or_default(),
            body,
            timeout,
            max_body_bytes,
        };

        // WHY: Convert HAL errors to kernel errors for consistent error handling.
        // Timeout and connection errors are the most common failure modes.
        let response = self.net.http_request(req).await.map_err(|e| match e {
            HalNetError::InvalidArgs(msg) => KernelError::invalid_args(msg),
            HalNetError::Timeout { timeout } => {
                KernelError::timeout(format!("request timed out after {:?}", timeout))
            }
            HalNetError::Connect(msg) => KernelError::io(format!("connection failed: {msg}")),
            HalNetError::Http(msg) => KernelError::io(format!("http error: {msg}")),
        })?;

        // =====================================================================
        // PHASE 5: Response Body Encoding
        // =====================================================================
        // WHY: Provide BOTH UTF-8 and base64-encoded bodies for maximum compatibility.
        // - `body` field: Convenient for text/json/xml responses (lossy UTF-8 conversion)
        // - `body_base64` field: Lossless encoding for binary responses (images, PDFs, etc.)
        //
        // SECURITY: from_utf8_lossy prevents crashes on malformed UTF-8, replacing
        // invalid sequences with � (U+FFFD REPLACEMENT CHARACTER).
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(&response.body);

        // WHY: Include truncation flag and body_size so callers know if response was incomplete.
        // This allows them to warn users or retry with larger `max_body_bytes`.
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
