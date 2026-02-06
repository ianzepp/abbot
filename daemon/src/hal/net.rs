//! Network HAL - Hardware Abstraction Layer for HTTP Operations
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module provides a trait-based abstraction for HTTP operations, wrapping
//! reqwest to provide bounded response handling and structured error types.
//! The HAL pattern enables testing with mock HTTP clients and provides a
//! consistent interface for network operations.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Bounded responses: Enforces limits on response body size to prevent OOM
//! - Streaming consumption: Reads response bodies incrementally, not all at once
//! - Timeout enforcement: Applies timeouts at both connection and read levels
//! - Error categorization: Distinguishes between connection, timeout, and HTTP errors
//!
//! TRADE-OFFS
//! ==========
//! - Body size limits: Responses exceeding max_body_bytes are truncated; callers
//!   must handle partial data. This prevents memory exhaustion but requires care.
//! - Eager truncation: Once the limit is hit, streaming stops immediately;
//!   we don't wait for the full response. This saves bandwidth but may confuse
//!   servers expecting full consumption.
//! - Header value conversion: Invalid UTF-8 in header values is silently converted
//!   to empty strings rather than failing the request.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

// =============================================================================
// TYPES
// =============================================================================
//
// HTTP request and response types provide a platform-agnostic representation
// of HTTP operations. These types insulate business logic from reqwest details.

#[derive(Debug, Clone)]
pub struct HalHttpRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
    pub max_body_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct HalHttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub truncated: bool,
    pub body_size: usize,
}

// =============================================================================
// ERRORS
// =============================================================================
//
// Network errors categorize failures to enable appropriate handling.
// Distinguishing timeouts from connection failures from HTTP errors allows
// callers to implement retry logic, failover, or user-facing error messages.

#[derive(Debug, Clone)]
pub enum HalNetError {
    InvalidArgs(String),
    Connect(String),
    Http(String),
    Timeout { timeout: Duration },
}

impl HalNetError {
    /// Create an invalid arguments error.
    ///
    /// WHY: Validates request parameters before attempting network I/O,
    /// failing fast on malformed inputs (empty URL, invalid headers).
    pub fn invalid_args(msg: impl Into<String>) -> Self {
        Self::InvalidArgs(msg.into())
    }

    /// Create an HTTP error.
    ///
    /// WHY: Wraps generic HTTP errors (protocol errors, server errors) in
    /// a domain-specific type for uniform error handling.
    pub fn http(e: impl std::fmt::Display) -> Self {
        Self::Http(e.to_string())
    }

    /// Create a connection error.
    ///
    /// WHY: Distinguishes connection failures (DNS, TCP handshake) from
    /// HTTP-level errors, enabling connection-specific retry strategies.
    pub fn connect(e: impl std::fmt::Display) -> Self {
        Self::Connect(e.to_string())
    }
}

impl std::fmt::Display for HalNetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HalNetError::InvalidArgs(msg) => write!(f, "{msg}"),
            HalNetError::Connect(msg) => write!(f, "{msg}"),
            HalNetError::Http(msg) => write!(f, "{msg}"),
            HalNetError::Timeout { timeout } => write!(f, "request timed out after {:?}", timeout),
        }
    }
}

impl std::error::Error for HalNetError {}

// =============================================================================
// TRAIT
// =============================================================================
//
// The HalNet trait defines the network abstraction.
//
// WHY trait: Enables testing with mock HTTP clients and potential future
// support for alternative HTTP libraries or connection pooling strategies.

#[async_trait]
pub trait HalNet: Send + Sync {
    /// Execute an HTTP request with bounded response handling.
    ///
    /// WHY bounded: Response bodies are read incrementally up to max_body_bytes
    /// to prevent memory exhaustion on large or malicious responses.
    ///
    /// SAFETY: Callers must check the `truncated` field in the response. If true,
    /// the body is incomplete and may not be valid (e.g., partial JSON).
    async fn http_request(&self, req: HalHttpRequest) -> Result<HalHttpResponse, HalNetError>;
}

// =============================================================================
// HOST IMPLEMENTATION
// =============================================================================
//
// HostHalNet wraps reqwest to provide production HTTP functionality with
// bounded response handling and timeout enforcement.

#[derive(Debug, Default, Clone)]
pub struct HostHalNet;

#[async_trait]
impl HalNet for HostHalNet {
    async fn http_request(&self, req: HalHttpRequest) -> Result<HalHttpResponse, HalNetError> {
        // -------------------------------------------------------------------------
        // PHASE 1: INPUT VALIDATION
        // Validate request parameters before attempting network I/O
        // -------------------------------------------------------------------------
        if req.url.trim().is_empty() {
            return Err(HalNetError::invalid_args("url is empty"));
        }
        if req.max_body_bytes == 0 {
            return Err(HalNetError::invalid_args("max_body_bytes must be > 0"));
        }

        let method = req.method.parse::<reqwest::Method>().map_err(|_| {
            HalNetError::invalid_args(format!("unsupported method: {}", req.method))
        })?;

        let mut headers = HeaderMap::new();
        for (k, v) in &req.headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| HalNetError::invalid_args(format!("invalid header name: {k}")))?;
            let value = HeaderValue::from_str(v)
                .map_err(|_| HalNetError::invalid_args(format!("invalid header value for {k}")))?;
            headers.insert(name, value);
        }

        // -------------------------------------------------------------------------
        // PHASE 2: CLIENT SETUP AND REQUEST DISPATCH
        // Build the HTTP client with timeout and headers, then send the request
        // -------------------------------------------------------------------------
        let client = reqwest::Client::builder()
            .timeout(req.timeout)
            .default_headers(headers)
            .build()
            .map_err(HalNetError::http)?;

        let mut r = client.request(method, &req.url);
        if let Some(body) = req.body {
            r = r.body(body);
        }

        let response = match r.send().await {
            Ok(r) => r,
            Err(e) => {
                // WHY categorize errors: Enables callers to distinguish timeouts
                // (retry with backoff) from connection failures (failover to backup)
                if e.is_timeout() {
                    return Err(HalNetError::Timeout {
                        timeout: req.timeout,
                    });
                }
                if e.is_connect() {
                    return Err(HalNetError::connect(e));
                }
                return Err(HalNetError::http(e));
            }
        };

        // -------------------------------------------------------------------------
        // PHASE 3: RESPONSE METADATA EXTRACTION
        // Capture status and headers before consuming the body stream
        // -------------------------------------------------------------------------
        let status = response.status().as_u16();

        // WHY to_str().unwrap_or(""): Header values should be valid strings, but
        // we prefer graceful degradation (empty string) over failing the request
        let resp_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        // -------------------------------------------------------------------------
        // PHASE 4: BOUNDED BODY CONSUMPTION
        // Stream the response body, enforcing size limits to prevent OOM
        // -------------------------------------------------------------------------
        let mut body: Vec<u8> = Vec::new();
        let mut truncated = false;
        let mut body_size: usize = 0;

        let mut stream = response.bytes_stream();
        use futures::StreamExt;
        while let Some(next) = stream.next().await {
            let chunk = match next {
                Ok(c) => c,
                Err(e) => {
                    // WHY handle timeout during streaming: The initial send() may
                    // succeed, but timeout can still occur while reading the body
                    if e.is_timeout() {
                        return Err(HalNetError::Timeout {
                            timeout: req.timeout,
                        });
                    }
                    return Err(HalNetError::http(e));
                }
            };

            body_size = body_size.saturating_add(chunk.len());

            if body.len() < req.max_body_bytes {
                let remaining = req.max_body_bytes - body.len();
                if chunk.len() <= remaining {
                    body.extend_from_slice(&chunk);
                } else {
                    // WHY partial chunk: Append as much as fits, then truncate
                    body.extend_from_slice(&chunk[..remaining]);
                    truncated = true;
                    break;
                }
            } else {
                truncated = true;
                break;
            }

            if body.len() >= req.max_body_bytes {
                // WHY mark truncated conservatively: We've consumed exactly the
                // limit. There may be more data in the stream, but we can't know
                // without reading. Mark as truncated to signal potential incompleteness.
                truncated = true;
                break;
            }
        }

        Ok(HalHttpResponse {
            status,
            headers: resp_headers,
            body,
            truncated,
            body_size,
        })
    }
}
