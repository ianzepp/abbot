use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use crate::bus::Hub;
use crate::history::Store;
use crate::llm::LlmClient;
use crate::tools::SharedCwd;
use super::{parse, strip_tags, Executor, SharedRegistry};

/// Maximum number of LLM iterations per message.
/// High limit as a circuit breaker; monks should self-regulate via garden/pray.
const MAX_ITERATIONS: usize = 50;

/// How long after activity before pings are processed again.
const PING_DEBOUNCE: Duration = Duration::from_secs(30);

/// Truncate output for display in context
fn truncate_output(s: &str, max_chars: usize) -> String {
    let s = s.trim().replace('\n', " ↵ ");
    if s.len() <= max_chars {
        s
    } else {
        format!("{}...", &s[..max_chars])
    }
}

/// A monk in the monastery.
///
/// Context is built from 5 layers:
/// - Layer 1 (System): Commandments, tools, base behavior - loaded from files
/// - Layer 2 (Self): Monk's identity, memories, relationships - stored in DB, monk can rewrite
/// - Layer 3 (Workspace): Per-channel role and focus - stored in DB per (monk, channel)
/// - Layer 4 (Messages): Recent channel history - queried from DB
pub struct Monk {
    /// Unique identifier (e.g., "brother-thomas")
    id: String,

    /// Currently active channel (interior mutability for concurrent access)
    active_channel: Mutex<Option<String>>,

    /// Database for persistence
    store: Arc<Store>,

    /// Layer 1: System prompt (loaded once)
    system: String,

    /// Working directory for tool execution (shared so cd tool can modify it)
    cwd: SharedCwd,

    /// LLM client (optional, set after construction)
    llm: Option<LlmClient>,

    /// Pub/sub hub for publishing messages
    hub: Option<Arc<RwLock<Hub>>>,

    /// Registry for tool access
    registry: Option<SharedRegistry>,

    /// Last activity timestamp (interior mutability for concurrent access)
    last_activity: Mutex<Option<Instant>>,
}

impl Monk {
    pub fn new(id: impl Into<String>, store: Arc<Store>, system: String) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        Self {
            id: id.into(),
            active_channel: Mutex::new(None),
            store,
            system,
            cwd: Arc::new(Mutex::new(cwd)),
            llm: None,
            hub: None,
            registry: None,
            last_activity: Mutex::new(None),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn active_channel(&self) -> Option<String> {
        self.active_channel.lock().unwrap().clone()
    }

    /// Set the LLM client for this monk.
    pub fn set_llm(&mut self, llm: LlmClient) {
        self.llm = Some(llm);
    }

    /// Set the pub/sub hub for this monk.
    pub fn set_hub(&mut self, hub: Arc<RwLock<Hub>>) {
        self.hub = Some(hub);
    }

    /// Set the registry for this monk.
    pub fn set_registry(&mut self, registry: SharedRegistry) {
        self.registry = Some(registry);
    }

    /// Set the working directory for this monk.
    pub fn set_cwd(&mut self, cwd: PathBuf) {
        *self.cwd.lock().unwrap() = cwd;
    }

    // === Layer 2: Self ===

    /// Read the monk's self layer (identity, memories, relationships)
    pub fn self_read(&self) -> String {
        self.store.get_monk_self(&self.id).unwrap_or_default()
    }

    /// Write the monk's self layer
    pub fn self_write(&self, content: &str) {
        let _ = self.store.set_monk_self(&self.id, content);
    }

    // === Layer 3: Workspace ===

    /// Read the workspace for the current channel
    pub fn workspace_read(&self) -> String {
        let channel = self.active_channel.lock().unwrap().clone();
        match channel {
            Some(ch) => self.store.get_workspace(&self.id, &ch).unwrap_or_default(),
            None => String::new(),
        }
    }

    /// Write the workspace for the current channel
    pub fn workspace_write(&self, content: &str) {
        let channel = self.active_channel.lock().unwrap().clone();
        if let Some(ch) = channel {
            let _ = self.store.set_workspace(&self.id, &ch, content);
        }
    }

    /// Switch to a different channel
    /// Returns the new context (layers 1-4 assembled)
    pub fn switch(&self, channel: &str) -> String {
        *self.active_channel.lock().unwrap() = Some(channel.to_string());
        self.build_context()
    }

    /// List all channels this monk has workspaces for
    pub fn channels(&self) -> Vec<String> {
        self.store.list_monk_channels(&self.id).unwrap_or_default()
    }

    // === Context Assembly ===

    /// Build the full context for the current state
    pub fn build_context(&self) -> String {
        let layer1 = &self.system;
        let layer2 = self.self_read();
        let layer3 = self.workspace_read();
        let layer4 = self.build_message_context();
        let layer5 = self.build_action_context();

        let mut result = String::new();

        // System prompt
        result.push_str(layer1);

        // Identity - tell the monk who it is, including current working directory
        let cwd = self.cwd.lock().unwrap();
        result.push_str(&format!(
            "\n\n<identity>\nYou are {}.\nWorking directory: {}\n</identity>",
            self.id,
            cwd.display()
        ));
        drop(cwd);

        if !layer2.is_empty() {
            result.push_str("\n\n<self>\n");
            result.push_str(&layer2);
            result.push_str("\n</self>");
        }

        if !layer3.is_empty() {
            result.push_str("\n\n<workspace>\n");
            result.push_str(&layer3);
            result.push_str("\n</workspace>");
        }

        if !layer4.is_empty() {
            result.push_str("\n\n<messages>\n");
            result.push_str(&layer4);
            result.push_str("\n</messages>");
        }

        if !layer5.is_empty() {
            result.push_str("\n\n<recent_actions>\n");
            result.push_str(&layer5);
            result.push_str("\n</recent_actions>");
        }

        result
    }

    /// Build layer 4: recent messages from active channel
    fn build_message_context(&self) -> String {
        let channel = self.active_channel.lock().unwrap().clone();
        let Some(channel) = channel else {
            return String::new();
        };

        let messages = self.store.recent_chat(&channel, 50).unwrap_or_default();

        messages
            .iter()
            .map(|m| {
                let text = m.text().unwrap_or("");
                format!("<{}> {}", m.sender, text)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Build layer 5: recent tool calls (what you did recently)
    fn build_action_context(&self) -> String {
        let tool_calls = match self.store.recent_tool_calls(&self.id, 30) {
            Ok(calls) => calls,
            Err(_) => return String::new(),
        };

        if tool_calls.is_empty() {
            return String::new();
        }

        // Group by batch_id to show logical groupings
        let mut current_batch = String::new();
        let mut lines = Vec::new();

        for tc in tool_calls.iter().rev() {
            // Show batch separator when batch changes
            if tc.batch_id != current_batch {
                if !current_batch.is_empty() {
                    lines.push(String::new()); // blank line between batches
                }
                current_batch = tc.batch_id.clone();
            }

            // Format: [tool] reason → truncated output
            let reason = tc.reason.as_deref().unwrap_or("no reason");
            let output = truncate_output(&tc.output, 150);
            lines.push(format!("[{}] {} → {}", tc.tool, reason, output));
        }

        lines.join("\n")
    }

    // === Lifecycle ===

    /// Delete this monk's data (dismissal)
    pub fn dismiss(&self) {
        let _ = self.store.delete_monk_self(&self.id);
        let _ = self.store.delete_all_workspaces(&self.id);
    }

    // === Message Handling ===

    /// Process an incoming message.
    /// This is called by the runner when a message arrives on a channel
    /// this monk is subscribed to.
    pub async fn on_message(&self, channel: &str, msg: &crate::bus::Message) {
        use crate::bus::MessageOp;

        // Check required fields first (without holding borrows)
        if self.llm.is_none() {
            tracing::warn!(monk = self.id, "no LLM client configured");
            return;
        }
        if self.hub.is_none() {
            tracing::warn!(monk = self.id, "no hub configured");
            return;
        }
        if self.registry.is_none() {
            tracing::warn!(monk = self.id, "no registry configured");
            return;
        }

        // Debounce pings if there was recent activity
        if msg.op == MessageOp::Ping {
            let last = *self.last_activity.lock().unwrap();
            if let Some(last) = last {
                if last.elapsed() < PING_DEBOUNCE {
                    tracing::trace!(
                        monk = self.id,
                        elapsed_secs = last.elapsed().as_secs(),
                        "skipping ping due to recent activity"
                    );
                    return;
                }
            }
        }

        // Update last activity for ALL messages (prevents races)
        *self.last_activity.lock().unwrap() = Some(Instant::now());

        // Switch to this channel's context if not already there
        if self.active_channel() != Some(channel.to_string()) {
            self.switch(channel);
        }

        // Now extract references
        let llm = self.llm.as_ref().unwrap();
        let hub = self.hub.as_ref().unwrap().clone();
        let registry = self.registry.as_ref().unwrap().clone();

        // Format the incoming message
        let incoming = match msg.op {
            MessageOp::Ping => {
                if let crate::bus::MessageData::Ping { tick, timestamp } = &msg.data {
                    format!("<ping tick=\"{}\" timestamp=\"{}\"/>", tick, timestamp)
                } else {
                    "<ping/>".to_string()
                }
            }
            MessageOp::Chat => {
                // Skip our own messages
                if msg.sender == self.id {
                    return;
                }
                let text = msg.text().unwrap_or("");
                format!("<{}> {}", msg.sender, text)
            }
            _ => {
                return;
            }
        };

        tracing::debug!(
            monk = self.id,
            channel,
            incoming = %incoming,
            "processing message"
        );

        // Create executor
        let executor = Executor::new(hub.clone(), self.store.clone(), registry.clone());

        // Generate batch ID for this on_message invocation
        let batch_id = uuid::Uuid::new_v4().to_string();

        // LLM loop with tool result feeding
        let mut user_message = incoming;
        for iteration in 0..MAX_ITERATIONS {
            // Build context
            let context = self.build_context();

            tracing::debug!(
                monk = self.id,
                iteration,
                context_len = context.len(),
                "calling LLM"
            );

            // Call LLM
            let response = match llm.chat(&context, &user_message, &[]).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(monk = self.id, error = %e, "LLM call failed");
                    return;
                }
            };

            tracing::debug!(
                monk = self.id,
                iteration,
                response_len = response.len(),
                "LLM response received"
            );

            // Log the actual response for debugging
            tracing::debug!(monk = self.id, response = %response, "LLM response content");

            // Parse response
            let mut parsed = parse(&response);
            tracing::debug!(
                monk = self.id,
                action_count = parsed.actions.len(),
                has_pong = parsed.has_pong,
                "parsed response"
            );

            // Check for pong (ping acknowledged, no action needed)
            if parsed.has_pong && parsed.actions.is_empty() {
                tracing::debug!(monk = self.id, "pong received, done");
                return;
            }

            // Fallback: if no say actions and no pong, treat non-empty text as implicit say
            // But only for chat messages, not pings (pings should be silent if no explicit action)
            let is_ping = msg.op == MessageOp::Ping;
            let has_say = parsed.actions.iter().any(|a| matches!(a, super::Action::Say { .. }));
            if !has_say && !parsed.has_pong && !is_ping {
                // Extract text that's not inside tags
                let text = strip_tags(&response);
                if !text.is_empty() {
                    tracing::debug!(monk = self.id, "implicit say fallback");
                    parsed.actions.push(super::Action::Say {
                        channel: channel.to_string(),
                        text,
                    });
                }
            }

            // Execute actions
            tracing::debug!(monk = self.id, "executing actions");
            let result = executor.execute(&parsed, &self.id, channel, self.cwd.clone(), &batch_id, iteration).await;
            tracing::debug!(monk = self.id, "actions executed");

            // Log what we did
            for tr in &result.tool_results {
                tracing::debug!(
                    monk = self.id,
                    tool = %tr.tool,
                    success = tr.success,
                    output_len = tr.output.len(),
                    "tool executed"
                );
            }
            for (ch, text) in &result.messages_sent {
                tracing::debug!(
                    monk = self.id,
                    channel = %ch,
                    text = %text,
                    "message sent"
                );
            }

            // If no tool results, we're done
            if result.tool_results.is_empty() {
                tracing::debug!(monk = self.id, "no tool results, done");
                return;
            }

            // Feed tool results back to LLM
            user_message = result.format_for_llm();
            tracing::debug!(
                monk = self.id,
                iteration,
                "feeding tool results back to LLM"
            );
        }

        tracing::warn!(
            monk = self.id,
            max_iterations = MAX_ITERATIONS,
            "reached max iterations"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> Arc<Store> {
        Arc::new(Store::open(":memory:").unwrap())
    }

    #[test]
    fn test_monk_self_layer() {
        let store = test_store();
        let monk = Monk::new("brother-thomas", store, "You are a monk.".into());

        assert_eq!(monk.self_read(), "");

        monk.self_write("I am Brother Thomas. I like debugging.");
        assert_eq!(monk.self_read(), "I am Brother Thomas. I like debugging.");
    }

    #[test]
    fn test_monk_workspace_layer() {
        let store = test_store();
        let mut monk = Monk::new("brother-thomas", store, "You are a monk.".into());

        // No active channel
        assert_eq!(monk.workspace_read(), "");

        // Switch to channel
        monk.switch("#project-x");
        assert_eq!(monk.workspace_read(), "");

        // Write workspace
        monk.workspace_write("Working on the auth bug. Need to check login.rs.");
        assert_eq!(monk.workspace_read(), "Working on the auth bug. Need to check login.rs.");

        // Switch away and back
        monk.switch("#general");
        assert_eq!(monk.workspace_read(), ""); // Different channel, no workspace yet

        monk.switch("#project-x");
        assert_eq!(monk.workspace_read(), "Working on the auth bug. Need to check login.rs.");
    }

    #[test]
    fn test_monk_context_assembly() {
        let store = test_store();
        let mut monk = Monk::new("brother-thomas", store, "You are a monk.".into());

        monk.self_write("I am Brother Thomas.");
        monk.switch("#general");
        monk.workspace_write("Observing the channel.");

        let context = monk.build_context();

        assert!(context.contains("You are a monk."));
        assert!(context.contains("<self>"));
        assert!(context.contains("I am Brother Thomas."));
        assert!(context.contains("<workspace>"));
        assert!(context.contains("Observing the channel."));
    }

    #[test]
    fn test_monk_dismiss() {
        let store = test_store();
        let mut monk = Monk::new("brother-thomas", store.clone(), "You are a monk.".into());

        monk.self_write("I am Brother Thomas.");
        monk.switch("#general");
        monk.workspace_write("Notes here.");

        monk.dismiss();

        // Data should be gone
        let monk2 = Monk::new("brother-thomas", store, "You are a monk.".into());
        assert_eq!(monk2.self_read(), "");
    }
}
