use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

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

#[derive(Debug, Clone)]
pub struct ExternalToolResult {
    pub tool_call_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug)]
pub enum TurnWaitError {
    Cancelled,
    NotFound,
}

#[derive(Debug)]
struct PendingTool {
    name: String,
    result: Option<ExternalToolResult>,
    notify: Arc<Notify>,
}

#[derive(Debug, Default)]
struct TurnState {
    cancelled: bool,
    cancelled_reason: Option<String>,
    pending: HashMap<String, PendingTool>,
    recent_completed: VecDeque<String>,
}

#[derive(Debug, Default)]
pub struct TurnRuntime {
    turns: Mutex<HashMap<TurnKey, TurnState>>,
}

impl TurnRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn ensure_turn(&self, key: &TurnKey) {
        let mut turns = self.turns.lock().await;
        turns.entry(key.clone()).or_insert_with(TurnState::default);
    }

    pub async fn cancel(&self, key: &TurnKey, reason: &str) {
        let mut turns = self.turns.lock().await;
        let state = turns.entry(key.clone()).or_insert_with(TurnState::default);
        state.cancelled = true;
        state.cancelled_reason = Some(reason.to_string());
        for pending in state.pending.values() {
            pending.notify.notify_waiters();
        }
    }

    pub async fn is_cancelled(&self, key: &TurnKey) -> bool {
        let turns = self.turns.lock().await;
        turns
            .get(key)
            .map(|s| s.cancelled)
            .unwrap_or(false)
    }

    pub async fn pending_tool_name(&self, key: &TurnKey, tool_call_id: &str) -> Option<String> {
        let turns = self.turns.lock().await;
        turns
            .get(key)
            .and_then(|s| s.pending.get(tool_call_id))
            .map(|p| p.name.clone())
    }

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
        pending.notify.notify_waiters();

        let key_sig = format!("{}:{}:{}", key.scope, key.reply_to, tool_call_id);
        state.recent_completed.push_back(key_sig);
        const MAX_RECENT: usize = 256;
        while state.recent_completed.len() > MAX_RECENT {
            state.recent_completed.pop_front();
        }
        Ok(())
    }

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
                    drop(turns);
                    let mut turns = self.turns.lock().await;
                    if let Some(state) = turns.get_mut(key) {
                        state.pending.remove(tool_call_id);
                    }
                    return Ok(result);
                }
                pending.notify.clone()
            };

            notify.notified().await;
        }
    }
}
