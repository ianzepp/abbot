// MindService provides periodic background monitoring and long-term memory.
//
// Unlike heads which respond to direct input, the mind operates on a timer,
// summarizing recent activity and potentially triggering actions based on
// patterns. It's designed for LTM (long-term memory) tasks like summarizing
// old conversations, archiving completed tasks, or detecting issues that
// weren't addressed during active conversation.

use std::sync::Arc;

use tokio::time::timeout;

use crate::bus::{MessageData, MessageOp, Scope};
use crate::history::Store;
use crate::llm::OpenAICompatClient;

use super::mind_parser::{parse_mind_response, LtmAction};
use super::{MindBundleBuilder, MindBundleConfig, MindConfig, RuntimeBus};

pub struct MindService {
    bus: RuntimeBus,
    store: Arc<Store>,
    head_id: String,
    scopes: Vec<Scope>,
    mind_cfg: MindConfig,
    llm: Option<Arc<OpenAICompatClient>>,
}

impl MindService {
    pub fn new(
        bus: RuntimeBus,
        store: Arc<Store>,
        head_id: impl Into<String>,
        scopes: Vec<Scope>,
    ) -> Self {
        let head_id = head_id.into();
        let mind_cfg = MindConfig::from_env();

        let llm = if mind_cfg.llm.enabled {
            tracing::info!(
                head = %head_id,
                base_url = %mind_cfg.llm.base_url,
                model = %mind_cfg.llm.model,
                api_key_set = !mind_cfg.llm.api_key.is_empty(),
                temperature = ?mind_cfg.llm.temperature,
                max_tokens = ?mind_cfg.llm.max_tokens,
                tick_interval = mind_cfg.tick_interval,
                "mind llm enabled via MIND_* env"
            );
            Some(Arc::new(OpenAICompatClient::new(
                &mind_cfg.llm.base_url,
                &mind_cfg.llm.api_key,
                &mind_cfg.llm.model,
                mind_cfg.llm.temperature,
                mind_cfg.llm.max_tokens,
                mind_cfg.llm.extra_headers.clone(),
            )))
        } else {
            tracing::info!(
                head = %head_id,
                "mind disabled (set MIND_MODEL to enable)"
            );
            None
        };

        Self {
            bus,
            store,
            head_id,
            scopes,
            mind_cfg,
            llm,
        }
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(&self) {
        let mut rx = self.bus.hub().read().await.subscribe_all();
        tracing::info!(head = %self.head_id, "mind service started");

        tracing::debug!(head = %self.head_id, "mind entering message loop");
        loop {
            tracing::trace!(head = %self.head_id, "mind waiting for message");
            let msg = match rx.recv().await {
                Ok(m) => {
                    tracing::trace!(head = %self.head_id, op = ?m.op, "mind received message");
                    m
                }
                Err(e) => {
                    tracing::warn!(head = %self.head_id, error = ?e, "mind recv error");
                    continue;
                }
            };

            // Only trigger on ping ticks
            if msg.op != MessageOp::Ping {
                continue;
            }

            let MessageData::Ping { tick, .. } = &msg.data else {
                continue;
            };

            // Check if this tick triggers reflection
            if self.mind_cfg.tick_interval == 0 {
                continue;
            }

            tracing::debug!(head = %self.head_id, tick = tick, interval = self.mind_cfg.tick_interval, "mind received ping");

            if tick % self.mind_cfg.tick_interval != 0 {
                continue;
            }

            tracing::info!(head = %self.head_id, tick = tick, "mind reflecting");
            self.reflect().await;
        }
    }

    async fn reflect(&self) {
        let Some(llm) = &self.llm else { return };

        let bundle_builder = MindBundleBuilder::new(self.store.clone());
        let bundle_cfg = MindBundleConfig::new(&self.head_id, self.scopes.clone());
        let messages = bundle_builder.build(&bundle_cfg);

        tracing::info!(
            head = %self.head_id,
            message_count = messages.len(),
            "mind thinking"
        );

        let result = match timeout(
            std::time::Duration::from_secs(120),
            llm.chat(messages),
        )
        .await
        {
            Ok(Ok(res)) => res,
            Ok(Err(e)) => {
                tracing::error!(head = %self.head_id, error = %e, "mind llm error");
                return;
            }
            Err(_) => {
                tracing::error!(head = %self.head_id, "mind llm timeout");
                return;
            }
        };

        tracing::info!(
            head = %self.head_id,
            "\n--- MIND RESPONSE ---\n{}\n--- END RESPONSE ---",
            result.content
        );

        let parsed = parse_mind_response(&result.content);

        if parsed.is_empty() {
            tracing::info!(head = %self.head_id, "mind produced no LTM changes");
            return;
        }

        // Apply LTM changes
        self.apply_ltm_changes(&parsed.actions).await;
    }

    async fn apply_ltm_changes(&self, actions: &[LtmAction]) {
        let current_ltm = self
            .store
            .get_head_ltm(&self.head_id)
            .unwrap_or_default();

        let mut ltm = current_ltm.clone();

        for action in actions {
            match action {
                LtmAction::Append(content) => {
                    if !ltm.is_empty() {
                        ltm.push_str("\n\n");
                    }
                    ltm.push_str(content);
                    tracing::info!(
                        head = %self.head_id,
                        content = %content,
                        "ltm append"
                    );
                }
                LtmAction::Replace { pattern, content } => {
                    if let Some(pos) = ltm.find(pattern) {
                        let end = pos + pattern.len();
                        ltm.replace_range(pos..end, content);
                        tracing::info!(
                            head = %self.head_id,
                            pattern = %pattern,
                            content = %content,
                            "ltm replace"
                        );
                    } else {
                        tracing::warn!(
                            head = %self.head_id,
                            pattern = %pattern,
                            "ltm replace: pattern not found"
                        );
                    }
                }
                LtmAction::Clear(pattern) => {
                    if ltm.contains(pattern) {
                        ltm = ltm.replace(pattern, "");
                        // Clean up double newlines
                        while ltm.contains("\n\n\n") {
                            ltm = ltm.replace("\n\n\n", "\n\n");
                        }
                        ltm = ltm.trim().to_string();
                        tracing::info!(
                            head = %self.head_id,
                            pattern = %pattern,
                            "ltm clear"
                        );
                    } else {
                        tracing::warn!(
                            head = %self.head_id,
                            pattern = %pattern,
                            "ltm clear: pattern not found"
                        );
                    }
                }
            }
        }

        // Save if changed
        if ltm != current_ltm {
            if let Err(e) = self.store.set_head_ltm(&self.head_id, &ltm) {
                tracing::error!(
                    head = %self.head_id,
                    error = %e,
                    "failed to save LTM"
                );
            } else {
                tracing::info!(
                    head = %self.head_id,
                    ltm_len = ltm.len(),
                    "ltm saved"
                );
            }
        }
    }
}
