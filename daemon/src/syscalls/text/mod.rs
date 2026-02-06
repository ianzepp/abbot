//! Text - Simple text echo and testing utilities
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The `text` namespace provides basic text manipulation and testing utilities for the Abbot
//! kernel. Currently it implements a single syscall (`text:echo`) primarily used for testing
//! syscall dispatch, frame protocol validation, and integration testing scenarios.
//!
//! **Core purpose:**
//! - Testing syscall infrastructure without side effects
//! - Validating Frame protocol (request → response flow)
//! - Debugging syscall dispatch and cancellation behavior
//! - Integration test harness for kernel communication
//!
//! **Integration points:**
//! - No kernel services (stateless, no Store/VFS/HAL dependencies)
//! - No persistence (pure in-memory operation)
//! - No external processes (unlike proc/git/net namespaces)
//! - Frame protocol only (req → ok)
//!
//! **Registered syscalls:**
//! - `text:echo` - Echo back provided text (identity operation for testing)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Minimal surface area**: Simplest possible syscall implementation for testing
//! - **No side effects**: Echo operations never modify kernel state or filesystem
//! - **Predictable behavior**: Input text = output text (deterministic for tests)
//! - **Stateless execution**: No shared state, no concurrency concerns
//! - **Test-first design**: Designed for testing infrastructure, not production use
//!
//! WHY TEXT:ECHO EXISTS
//! ====================
//! Every syscall requires the following infrastructure to work correctly:
//! - Frame serialization/deserialization (JSON parsing)
//! - Syscall dispatch routing (name → handler mapping)
//! - Cancellation propagation (tokio::select! on ctx.cancel)
//! - Channel communication (mpsc::Sender<Frame>)
//! - Error handling (Result<(), KernelError>)
//!
//! `text:echo` provides a minimal syscall that exercises all these code paths without:
//! - Database access (unlike state/want/task namespaces)
//! - Filesystem operations (unlike fs/patch namespaces)
//! - External processes (unlike proc/git namespaces)
//! - Network requests (unlike net namespace)
//! - Complex business logic (unlike llm/chat/room namespaces)
//!
//! This makes it ideal for:
//! - Unit testing syscall dispatch infrastructure
//! - Integration testing frame protocol
//! - Performance benchmarking syscall overhead
//! - Debugging cancellation behavior
//!
//! SECURITY MODEL
//! ==============
//! - **No actor restrictions**: Any actor may call text:echo (no mutation guard)
//! - **No data validation**: Echo accepts any string (no injection risks since output = input)
//! - **No resource limits**: Unbounded text size (caller responsible for limiting payload)
//! - **No state access**: Cannot leak kernel state (no Store/VFS/HAL access)
//!
//! WHY NO SECURITY RESTRICTIONS:
//! Text:echo is a pure function with no side effects. Even if called with malicious input:
//! - Cannot modify filesystem (no VFS access)
//! - Cannot execute code (no proc/git invocation)
//! - Cannot access database (no Store access)
//! - Cannot leak secrets (output = input, no cross-session data)
//!
//! PERFORMANCE
//! ===========
//! - No I/O operations (pure in-memory string copy)
//! - No serialization overhead (text is already JSON string)
//! - No database queries or network requests
//! - Latency: <1μs for typical payloads (<1KB)
//!
//! CONCURRENCY
//! ===========
//! - Stateless execution (no shared state, no locking)
//! - Safe for concurrent calls (each invocation is independent)
//! - No lane assignment preference (can run on any lane)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Unbounded Text Size**
//!    - CHOSEN: No size limits on echo input
//!    - REJECTED: Truncation at fixed length (e.g., 10KB)
//!    - WHY: Testing may require large payloads (e.g., testing Frame serialization limits)
//!    - IMPLICATION: Caller must ensure reasonable payload sizes (no kernel enforcement)
//!
//! 2. **Test Utility vs. Production Feature**
//!    - CHOSEN: Designed for testing, not production use
//!    - WHY: No production use case for echoing text through syscall layer
//!    - IMPLICATION: May be removed or moved to test-only builds in future

mod echo;

pub use echo::TextEcho;

use crate::kernel::KernelDispatcher;
use std::sync::Arc;

/// Register all text namespace syscalls with the kernel dispatcher.
///
/// WHY: Centralizes syscall registration for the text namespace. Currently
/// only registers `text:echo`, but structured for future expansion if needed.
///
/// REGISTERED SYSCALLS:
/// - `text:echo` - Echo back provided text (testing/debugging utility)
pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(TextEcho::new()));
}
