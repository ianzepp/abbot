use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::bus::Hub;
use crate::history::Store;
use crate::llm::LlmClient;
use super::{parse, strip_tags, Executor, SharedRegistry};

/// Maximum number of LLM iterations per message.
const MAX_ITERATIONS: usize = 5;

/// A monk in the monastery.
///
/// Context is built from 4 layers:
/// - Layer 1 (System): Commandments, tools, base behavior - loaded from files
/// - Layer 2 (Self): Monk's identity, memories, relationships - stored in DB, monk can rewrite
/// - Layer 3 (Workspace): Per-channel role and focus - stored in DB per (monk, channel)
/// - Layer 4 (Messages): Recent channel history - queried from DB
pub struct Monk {
    /// Unique identifier (e.g., "brother-thomas")
    id: String,

    /// Currently active channel
    active_channel: Option<String>,

    /// Database for persistence
    store: Arc<Store>,

    /// Layer 1: System prompt (loaded once)
    system: String,

    /// Working directory for tool execution
    cwd: PathBuf,

    /// LLM client (optional, set after construction)
    llm: Option<LlmClient>,

    /// Pub/sub hub for publishing messages
    hub: Option<Arc<RwLock<Hub>>>,

    /// Registry for tool access
    registry: Option<SharedRegistry>,
}

impl Monk {
    pub fn new(id: impl Into<String>, store: Arc<Store>, system: String) -> Self {
        Self {
            id: id.into(),
            active_channel: None,
            store,
            system,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            llm: None,
            hub: None,
            registry: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn active_channel(&self) -> Option<&str> {
        self.active_channel.as_deref()
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
        self.cwd = cwd;
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
        match &self.active_channel {
            Some(channel) => self.store.get_workspace(&self.id, channel).unwrap_or_default(),
            None => String::new(),
        }
    }

    /// Write the workspace for the current channel
    pub fn workspace_write(&self, content: &str) {
        if let Some(channel) = &self.active_channel {
            let _ = self.store.set_workspace(&self.id, channel, content);
        }
    }

    /// Switch to a different channel
    /// Returns the new context (layers 1-4 assembled)
    pub fn switch(&mut self, channel: &str) -> String {
        self.active_channel = Some(channel.to_string());
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

        let mut parts = vec![layer1.as_str()];

        if !layer2.is_empty() {
            parts.push("<self>");
            parts.push(&layer2);
            parts.push("</self>");
        }

        if !layer3.is_empty() {
            parts.push("<workspace>");
            parts.push(&layer3);
            parts.push("</workspace>");
        }

        if !layer4.is_empty() {
            parts.push("<messages>");
            parts.push(&layer4);
            parts.push("</messages>");
        }

        parts.join("\n\n")
    }

    /// Build layer 4: recent messages from active channel
    fn build_message_context(&self) -> String {
        let Some(channel) = &self.active_channel else {
            return String::new();
        };

        let messages = self.store.recent_chat(channel, 50).unwrap_or_default();

        messages
            .iter()
            .map(|m| {
                let text = m.text().unwrap_or("");
                format!("<{}> {}", m.sender, text)
            })
            .collect::<Vec<_>>()
            .join("\n")
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
    pub async fn on_message(&mut self, channel: &str, msg: &crate::bus::Message) {
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

        // Switch to this channel's context if not already there (mutable borrow)
        if self.active_channel.as_deref() != Some(channel) {
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
            let result = executor.execute(&parsed, &self.id, channel, &self.cwd).await;
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
