//! Net - HTTP/HTTPS requests with protocol-level security
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The `net` namespace provides controlled HTTP/HTTPS request execution through the
//! `net:fetch` syscall. It integrates with the Hardware Abstraction Layer (HAL) via the
//! `HalNet` trait for platform-specific HTTP client implementation.
//!
//! **Security model:**
//! This namespace balances functionality with protection against network-based attacks:
//! - **Protocol allowlist**: Only HTTP/HTTPS permitted (no file://, ftp://, etc.)
//! - **Response size limiting**: Bounded body size prevents memory exhaustion
//! - **Timeout enforcement**: Hung connections terminated after deadline
//! - **No actor restrictions**: All agents may fetch (HTTP GET is inherently read-only)
//! - **Binary-safe encoding**: Base64 support for non-UTF8 responses
//!
//! **Integration points:**
//! Unlike `proc` and `git`, this namespace does NOT require actor-based mutation permission
//! because HTTP requests do not modify local filesystem state. This allows "hand" agents
//! (controlled by LLMs) to fetch documentation, check service status, and retrieve external
//! data without elevated privileges.
//!
//! **Registered syscalls:**
//! - `net:fetch` - Execute HTTP/HTTPS request with security constraints
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Read-only by nature**: HTTP GET cannot mutate local state
//! - **Protocol-level security**: Validate URL scheme before network access
//! - **Fail-safe defaults**: Conservative timeout (30s) and size limit (10MB)
//! - **Binary-safe**: Base64 encoding for images, PDFs, etc.
//! - **Transparent truncation**: Caller knows if response was incomplete

mod fetch;

pub use fetch::NetFetch;
