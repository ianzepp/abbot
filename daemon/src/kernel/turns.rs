//! Turn Runtime - Kernel-owned turn state and external tool rendezvous
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The turn runtime is the single source of truth for turn lifecycle state
//! keyed by (scope, reply_to). It owns:
//! - Turn cancellation state for a user-visible conversation exchange
//! - External tool call rendezvous: pending tool calls and result delivery
//! - Recent completion tracking (anti-replay, debugging)
//!
//! This module exists to enforce syscall-driven chat semantics from the
//! refactor spec: external tools block a head's active need until results
//! arrive, rather than creating a new need. The rendezvous mechanism ensures
//! tool results wake the correct waiting head.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Turn state is kernel-owned: only chat:* syscalls mutate this state
//! - External tool calls are rendezvous points, not new work items
//! - Cancellation is graceful: notify waiting heads so they can clean up
//! - Recent completion tracking prevents duplicate result delivery

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

// =============================================================================
// TYPES
// =============================================================================

/// Turn identifier (scope, reply_to).
///
/// WHY this exists: Turns are the user-visible unit of conversation exchange.
/// A turn may span multiple segments (HTTP connections) if external tools are
/// required. TurnKey provides a stable identity across segments.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TurnKey {
    pub scope: String,
    pub reply_to: Uuid,
}

impl TurnKey {
    pub fn new(scope: impl Into<String>, reply_to: Uuid) -> Self {
        Self {
            scope: scope.into(),
            reply_to,
        }
    }
}

/// External tool call result payload.
///
/// WHY this exists: External tool calls (user__*) execute client-side and
/// return results asynchronously. This structure carries the result back to
/// the waiting head.
#[derive(Debug, Clone)]
pub struct ExternalToolResult {
    pub tool_call_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
}

/// Error type for turn wait operations.
///
/// WHY separate from KernelError: Turn wait is a rendezvous coordination
/// primitive; the caller must distinguish cancellation from missing state.
#[derive(Debug)]
pub enum TurnWaitError {
    Cancelled,
    NotFound,
}

/// Internal rendezvous state for a single pending external tool call.
///
/// WHY internal: This is kernel coordination machinery; heads interact via
/// the TurnRuntime API, not directly with PendingTool.
#[derive(Debug)]
struct PendingTool {
    name: String,
    result: Option<ExternalToolResult>,
    notify: Arc<Notify>,
}

/// Per-turn kernel state.
///
/// WHY internal: TurnRuntime encapsulates all turn state mutations to enforce
/// syscall-driven semantics.
#[derive(Debug, Default)]
struct TurnState {
    cancelled: bool,
    cancelled_reason: Option<String>,
    pending: HashMap<String, PendingTool>,
    recent_completed: VecDeque<String>,
}

// =============================================================================
// TURN RUNTIME
// =============================================================================

/// Kernel-owned turn lifecycle and external tool rendezvous.
///
/// WHY this exists: The syscall refactor requires a single canonical owner of
/// turn state to coordinate external tool resumption (same need, not new need)
/// and graceful cancellation.
///
/// CONCURRENCY
/// -----------
/// All methods lock the full turn map. This is acceptable because turn
/// operations are infrequent (user-initiated chat) and lock hold times are
/// minimal (no I/O under lock).
#[derive(Debug, Default)]
pub struct TurnRuntime {
    turns: Mutex<HashMap<TurnKey, TurnState>>,
}

impl TurnRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ensure a turn exists for the given key.
    ///
    /// WHY: Ingress must ensure a turn stream exists before enqueueing work
    /// so head output is never dropped.
    pub async fn ensure_turn(&self, key: &TurnKey) {
        let mut turns = self.turns.lock().await;
        turns.entry(key.clone()).or_insert_with(TurnState::default);
    }

    /// Cancel a turn and wake all waiting heads.
    ///
    /// WHY: Client disconnect (chat:cancel) should notify in-flight heads so
    /// they can exit gracefully rather than completing work that will be
    /// discarded.
    pub async fn cancel(&self, key: &TurnKey, reason: &str) {
        let mut turns = self.turns.lock().await;
        let state = turns.entry(key.clone()).or_insert_with(TurnState::default);
        state.cancelled = true;
        state.cancelled_reason = Some(reason.to_string());

        // WHY wake all: Heads blocked on external tools must observe cancellation
        for pending in state.pending.values() {
            pending.notify.notify_waiters();
        }
    }

    /// Remove all runtime state for a completed terminal turn.
    ///
    /// WHY: Prevent unbounded growth of turn state in long-running daemons.
    pub async fn finish(&self, key: &TurnKey) {
        let mut turns = self.turns.lock().await;
        turns.remove(key);
    }

    /// Check if a turn is cancelled.
    ///
    /// WHY: Heads check cancellation before expensive operations (LLM calls,
    /// tool dispatch) to avoid wasted work.
    pub async fn is_cancelled(&self, key: &TurnKey) -> bool {
        let turns = self.turns.lock().await;
        turns.get(key).map(|s| s.cancelled).unwrap_or(false)
    }

    /// Look up the name of a pending external tool call.
    ///
    /// WHY: Diagnostic support for monitoring/debugging pending tool state.
    pub async fn pending_tool_name(&self, key: &TurnKey, tool_call_id: &str) -> Option<String> {
        let turns = self.turns.lock().await;
        turns
            .get(key)
            .and_then(|s| s.pending.get(tool_call_id))
            .map(|p| p.name.clone())
    }

    /// Register a pending external tool call.
    ///
    /// WHY: chat:tool syscall registers pending state before emitting the tool
    /// call to the client. This creates a rendezvous point for the head to
    /// await the result.
    ///
    /// SECURITY NOTE: Duplicate tool_call_id indicates a protocol violation
    /// (client replay or head bug). Reject to prevent rendezvous confusion.
    pub async fn register_external_tool(
        &self,
        key: &TurnKey,
        tool_call_id: &str,
        name: &str,
    ) -> Result<(), String> {
        let mut turns = self.turns.lock().await;
        let state = turns.entry(key.clone()).or_insert_with(TurnState::default);
        if state.pending.contains_key(tool_call_id) {
            return Err(format!("duplicate pending tool call: {tool_call_id}"));
        }
        state.pending.insert(
            tool_call_id.to_string(),
            PendingTool {
                name: name.to_string(),
                result: None,
                notify: Arc::new(Notify::new()),
            },
        );
        Ok(())
    }

    /// Deliver a tool result and wake waiting head.
    ///
    /// WHY: chat:tool_result syscall delivers the result and unblocks the head
    /// so it can continue the same leased need (continuation, not new need).
    ///
    /// TRADE-OFF: Recent completion tracking allows diagnostic queries but
    /// consumes memory. Limit to 256 entries per turn to bound overhead.
    pub async fn deliver_external_tool_result(
        &self,
        key: &TurnKey,
        tool_call_id: &str,
        name: &str,
        content: String,
        is_error: bool,
    ) -> Result<(), String> {
        let mut turns = self.turns.lock().await;
        let state = turns.entry(key.clone()).or_insert_with(TurnState::default);
        let Some(pending) = state.pending.get_mut(tool_call_id) else {
            return Err(format!("no pending tool call for id: {tool_call_id}"));
        };
        pending.result = Some(ExternalToolResult {
            tool_call_id: tool_call_id.to_string(),
            name: name.to_string(),
            content,
            is_error,
        });

        // WHY notify: Wake the head blocked on this tool result
        pending.notify.notify_waiters();

        // WHY track recent completions: Debugging, anti-replay, observability
        let key_sig = format!("{}:{}:{}", key.scope, key.reply_to, tool_call_id);
        state.recent_completed.push_back(key_sig);
        const MAX_RECENT: usize = 256;
        while state.recent_completed.len() > MAX_RECENT {
            state.recent_completed.pop_front();
        }
        Ok(())
    }

    /// Wait for and consume an external tool result.
    ///
    /// WHY: Heads block here after emitting chat:tool, resuming only when the
    /// client submits chat:tool_result. This enforces the "same need" semantic
    /// from the refactor spec.
    ///
    /// CONCURRENCY: Lock is released while waiting (notify.notified() is
    /// called outside the critical section) to avoid blocking other turns.
    pub async fn take_external_tool_result(
        &self,
        key: &TurnKey,
        tool_call_id: &str,
    ) -> Result<ExternalToolResult, TurnWaitError> {
        loop {
            let notify = {
                let turns = self.turns.lock().await;
                let Some(state) = turns.get(key) else {
                    return Err(TurnWaitError::NotFound);
                };
                if state.cancelled {
                    return Err(TurnWaitError::Cancelled);
                }
                let Some(pending) = state.pending.get(tool_call_id) else {
                    return Err(TurnWaitError::NotFound);
                };
                if let Some(result) = pending.result.clone() {
                    // WHY drop lock before reacquiring: Result is ready, remove pending
                    // state and return. Drop read lock before acquiring write lock.
                    drop(turns);
                    let mut turns = self.turns.lock().await;
                    if let Some(state) = turns.get_mut(key) {
                        state.pending.remove(tool_call_id);
                    }
                    return Ok(result);
                }
                pending.notify.clone()
            };

            // WHY await outside lock: Notify may take arbitrarily long (waiting
            // for client to submit result). Release lock to avoid blocking other
            // turn operations.
            notify.notified().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finish_removes_turn_state() {
        let turns = TurnRuntime::new();
        let key = TurnKey::new("main", Uuid::new_v4());
        turns.ensure_turn(&key).await;
        assert!(!turns.is_cancelled(&key).await);

        turns.cancel(&key, "test").await;
        assert!(turns.is_cancelled(&key).await);

        turns.finish(&key).await;
        assert!(!turns.is_cancelled(&key).await);
    }
}
