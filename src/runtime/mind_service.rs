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
use crate::agent_tools::{exec_mind_tool, mind_tool_specs};

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
            self.reflect(*tick).await;
        }
    }

    async fn reflect(&self, tick: u64) {
        let Some(llm) = &self.llm else { return };

        let bundle_builder = MindBundleBuilder::new(self.store.clone());
        let bundle_cfg = MindBundleConfig::new(&self.head_id, self.scopes.clone());
        let mut messages = bundle_builder.build(&bundle_cfg);

        tracing::info!(
            head = %self.head_id,
            message_count = messages.len(),
            "mind thinking"
        );

        let run_id = format!("{}:{}", self.head_id, tick);
        let tools = mind_tool_specs();
        let tool_choice = Some(serde_json::json!("auto"));

        for iter in 0..6usize {
            let result = match timeout(
                std::time::Duration::from_secs(120),
                llm.chat_with_tools(messages.clone(), Some(tools.clone()), tool_choice.clone()),
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

            let _ = self.store.log_llm_interaction(
                "mind",
                &run_id,
                iter,
                &result.request_json,
                &result.response_json,
            );

            if result.tool_calls.is_empty() {
                break;
            }

            messages.push(crate::llm::ChatMessage::assistant_tool_calls(
                result.tool_calls.clone(),
            ));
            for tc in &result.tool_calls {
                let out = exec_mind_tool(
                    self.store.as_ref(),
                    &self.head_id,
                    &tc.function.name,
                    &tc.function.arguments,
                )
                .await;
                messages.push(crate::llm::ChatMessage::tool_result(tc.id.clone(), out));
            }
        }
    }
}
