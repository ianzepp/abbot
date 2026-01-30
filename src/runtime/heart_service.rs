use std::sync::Arc;

use tokio::time::timeout;

use crate::bus::{MessageData, MessageOp, Scope};
use crate::history::Store;
use crate::llm::OpenAICompatClient;

use super::heart_parser::{parse_heart_response, LtmAction};
use super::{HeartBundleBuilder, HeartBundleConfig, HeartConfig, RuntimeBus};

pub struct HeartService {
    bus: RuntimeBus,
    store: Arc<Store>,
    head_id: String,
    scopes: Vec<Scope>,
    heart_cfg: HeartConfig,
    llm: Option<Arc<OpenAICompatClient>>,
}

impl HeartService {
    pub fn new(
        bus: RuntimeBus,
        store: Arc<Store>,
        head_id: impl Into<String>,
        scopes: Vec<Scope>,
    ) -> Self {
        let head_id = head_id.into();
        let heart_cfg = HeartConfig::from_env();

        let llm = if heart_cfg.llm.enabled {
            tracing::info!(
                head = %head_id,
                base_url = %heart_cfg.llm.base_url,
                model = %heart_cfg.llm.model,
                api_key_set = !heart_cfg.llm.api_key.is_empty(),
                temperature = ?heart_cfg.llm.temperature,
                max_tokens = ?heart_cfg.llm.max_tokens,
                tick_interval = heart_cfg.tick_interval,
                "heart llm enabled via HEART_* env"
            );
            Some(Arc::new(OpenAICompatClient::new(
                &heart_cfg.llm.base_url,
                &heart_cfg.llm.api_key,
                &heart_cfg.llm.model,
                heart_cfg.llm.temperature,
                heart_cfg.llm.max_tokens,
                heart_cfg.llm.extra_headers.clone(),
            )))
        } else {
            tracing::info!(
                head = %head_id,
                "heart disabled (set HEART_MODEL to enable)"
            );
            None
        };

        Self {
            bus,
            store,
            head_id,
            scopes,
            heart_cfg,
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
        tracing::info!(head = %self.head_id, "heart service started");

        tracing::debug!(head = %self.head_id, "heart entering message loop");
        loop {
            tracing::trace!(head = %self.head_id, "heart waiting for message");
            let msg = match rx.recv().await {
                Ok(m) => {
                    tracing::trace!(head = %self.head_id, op = ?m.op, "heart received message");
                    m
                }
                Err(e) => {
                    tracing::warn!(head = %self.head_id, error = ?e, "heart recv error");
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
            if self.heart_cfg.tick_interval == 0 {
                continue;
            }

            tracing::debug!(head = %self.head_id, tick = tick, interval = self.heart_cfg.tick_interval, "heart received ping");

            if tick % self.heart_cfg.tick_interval != 0 {
                continue;
            }

            tracing::info!(head = %self.head_id, tick = tick, "heart reflecting");
            self.reflect().await;
        }
    }

    async fn reflect(&self) {
        let Some(llm) = &self.llm else { return };

        let bundle_builder = HeartBundleBuilder::new(self.store.clone());
        let bundle_cfg = HeartBundleConfig::new(&self.head_id, self.scopes.clone());
        let messages = bundle_builder.build(&bundle_cfg);

        tracing::info!(
            head = %self.head_id,
            message_count = messages.len(),
            "heart thinking"
        );

        let result = match timeout(
            std::time::Duration::from_secs(120),
            llm.chat(messages),
        )
        .await
        {
            Ok(Ok(res)) => res,
            Ok(Err(e)) => {
                tracing::error!(head = %self.head_id, error = %e, "heart llm error");
                return;
            }
            Err(_) => {
                tracing::error!(head = %self.head_id, "heart llm timeout");
                return;
            }
        };

        tracing::info!(
            head = %self.head_id,
            "\n--- HEART RESPONSE ---\n{}\n--- END RESPONSE ---",
            result.content
        );

        let parsed = parse_heart_response(&result.content);

        if parsed.is_empty() {
            tracing::info!(head = %self.head_id, "heart produced no LTM changes");
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
