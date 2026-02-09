//! HeadService - Thin router that leases needs and routes them through rooms.
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! HeadService leases interactive needs (with reply_to) from NeedKernel, builds
//! a Door for client communication, and routes them through the RoomRegistry.
//! The actual LLM loop and tool execution happen inside the room runner, not here.
//!
//! All LLM lifecycles go through rooms. HeadService is the entry point that
//! bridges the need system with the room system for interactive chat.
//!
//! WHAT HEADSERVICE KEEPS:
//! - Need leasing from NeedKernel
//! - External tool loading from Store
//! - Door construction
//! - Need fulfillment
//!
//! WHAT MOVED TO ROOMS:
//! - LLM loop (room runner + door-aware agent loop)
//! - External tool coordination (Door methods)
//! - Chat emission (Door methods)
//! - Session write locks (Door field)

mod bundle;
mod config;
mod dispatch;
mod types;

// Re-exports for runtime/mod.rs
pub use bundle::{HeadBundleBuilder, HeadBundleConfig};
pub use config::HeadConfig;

use types::ActiveNeed;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use crate::Scope;
use crate::ems::EmsHandle;
use crate::hal::llm::{OpenAICompatClient, ToolSpec};
use crate::history::Store;
use crate::runtime::Kernel;
use crate::runtime::room::door::WebSocketDoor;
use crate::runtime::{Room, RoomAgent, RoomRunner, RoomType};
use crate::runtime::{SessionWriteLocks, SnapshotManager};
use crate::syscalls::dispatch::head_room_catalog;

/// HeadService routes interactive needs through rooms via the RoomRegistry.
pub struct HeadService {
    store: Arc<Store>,
    head_id: String,
    scopes: Vec<Scope>,
    llm: Option<Arc<OpenAICompatClient>>,
    workspace_root: PathBuf,
    snapshot: Arc<SnapshotManager>,
    session_locks: SessionWriteLocks,
    ems: Option<EmsHandle>,
}

impl HeadService {
    /// Create a new HeadService.
    pub fn new(
        store: Arc<Store>,
        workspace_root: PathBuf,
        head_id: impl Into<String>,
        scopes: Vec<Scope>,
        snapshot: Arc<SnapshotManager>,
        session_locks: SessionWriteLocks,
    ) -> Self {
        let head_id = head_id.into();
        let head_cfg = HeadConfig::from_config();

        let llm = if head_cfg.llm.enabled {
            tracing::debug!(
                head = %head_id,
                base_url = %head_cfg.llm.base_url,
                model = %head_cfg.llm.model,
                api_key_set = !head_cfg.llm.api_key.is_empty(),
                temperature = ?head_cfg.llm.temperature,
                max_tokens = ?head_cfg.llm.max_tokens,
                heartbeat_tick = head_cfg.heartbeat_tick,
                "head llm config"
            );
            Some(Arc::new(OpenAICompatClient::new(
                &head_cfg.llm.base_url,
                &head_cfg.llm.api_key,
                &head_cfg.llm.model,
                head_cfg.llm.temperature,
                head_cfg.llm.max_tokens,
                head_cfg.llm.extra_headers.clone(),
            )))
        } else {
            tracing::warn!(
                head = %head_id,
                model = %head_cfg.llm.model,
                base_url = %head_cfg.llm.base_url,
                api_key_set = !head_cfg.llm.api_key.is_empty(),
                "head llm disabled (configure head.model and providers.<provider>.base_url in abbot.toml)"
            );
            None
        };

        Self {
            store,
            head_id,
            scopes,
            llm,
            workspace_root,
            snapshot,
            session_locks,
            ems: None,
        }
    }

    pub fn with_ems(mut self, ems: EmsHandle) -> Self {
        self.ems = Some(ems);
        self
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    /// Main run loop: lease needs, route them through rooms.
    async fn run(self: Arc<Self>) {
        tracing::debug!(head = %self.head_id, "head service started");

        loop {
            // Lease a need with reply_to (interactive)
            let need = match self.lease_need().await {
                Some(n) => n,
                None => continue,
            };

            tracing::debug!(
                head = %self.head_id,
                need_id = %need.need_id,
                scope = %need.scope.as_deref().unwrap_or("main"),
                reply_to = ?need.reply_to,
                "processing need via room"
            );

            if self.llm.is_none() {
                let msg = "Head LLM not configured. Check abbot.toml: ensure head.model (or harness.model) is set and providers.<provider>.base_url is configured.";
                self.send_error(&need, msg).await;
                self.fulfill_need(&need, "LLM not configured").await;
                continue;
            }

            let Some(reply_to) = need.reply_to else {
                self.fulfill_need(&need, "No reply_to").await;
                continue;
            };

            let scope = need
                .scope
                .as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "main".to_string());

            // Load external tools from store
            let (external_tools, external_names) = self.load_external_tools(&scope).await;

            // Build Door for client communication
            let door: Arc<dyn crate::runtime::room::door::Door> = Arc::new(WebSocketDoor {
                scope: scope.clone(),
                thread_id: reply_to,
                actor: format!("head/{}", self.head_id),
                workspace: self.workspace_root.clone(),
                external_tool_specs: external_tools,
                external_names,
                session_locks: self.session_locks.clone(),
            });

            // Build the agent's initial messages via HeadBundleBuilder
            let initial_messages = self.build_head_messages(&need, &scope).await;

            // Build head agent system prompt (first system message from bundle)
            let system_prompt: String = initial_messages
                .first()
                .and_then(|m| m.content.clone())
                .unwrap_or_default();

            let Some(k) = Kernel::get() else {
                self.send_error(&need, "Kernel not initialized").await;
                self.fulfill_need(&need, "Kernel not initialized").await;
                continue;
            };

            // Get or create room via registry
            let head_id = self.head_id.clone();
            let store = self.store.clone();
            let scope_clone = scope.clone();
            let initial_msgs = initial_messages.clone();
            let sys_prompt = system_prompt.clone();

            let active = k
                .rooms()
                .get_or_create(&scope, move || {
                    let mut agent =
                        RoomAgent::new(head_id.clone(), "head", sys_prompt, head_room_catalog());
                    // Pre-populate agent with bundle-built messages (system + history)
                    agent.messages = initial_msgs;

                    let room = Room::new(
                        uuid::Uuid::new_v4().to_string(),
                        scope_clone.clone(),
                        RoomType::General,
                        "Interactive chat",
                        vec![agent],
                        12, // max rounds per turn
                    );

                    let runner = RoomRunner::new(store, &format!("room/{scope_clone}"));
                    (room, runner)
                })
                .await;

            // Attach door to the room
            k.rooms().attach_door(&scope, door).await;

            // Inject the need as a user message
            let need_prompt = format!(
                "You have been assigned a need to address:\n\n{}\n\nContext: {}",
                need.need_text,
                if need.context.is_empty() {
                    "(none)"
                } else {
                    &need.context
                }
            );
            k.rooms().inject_message(&scope, need_prompt).await;

            // Wait for room to finish processing
            k.rooms().wait_for_done(&scope).await;

            // Detach door
            k.rooms().detach_door(&scope).await;

            // Fulfill the need
            let summary = {
                let room = active.room.lock().await;
                room.transcript
                    .last()
                    .map(|t| t.content.clone())
                    .unwrap_or_else(|| "Completed".to_string())
            };
            self.fulfill_need(&need, &summary).await;
        }
    }

    /// Lease a need with reply_to set (interactive need).
    async fn lease_need(&self) -> Option<ActiveNeed> {
        let k = Kernel::get()?;
        let dispatcher = k.dispatcher().await;
        let req = crate::kernel::Frame::req(
            "need:lease",
            json!({
                "filter": {
                    "reply_to": {"$ne": null},
                }
            }),
        )
        .with_actor(format!("head/{}", self.head_id));
        let mut rx = dispatcher.dispatch(
            req,
            self.workspace_root.clone(),
            tokio_util::sync::CancellationToken::new(),
        );

        let frame = rx.recv().await?;
        if frame.op != crate::kernel::FrameOp::Ok {
            return None;
        }
        let v = frame.data?;

        ActiveNeed::try_from_value(&v)
    }

    /// Load external (user__*) tools from the store for a scope.
    async fn load_external_tools(&self, scope: &str) -> (Vec<ToolSpec>, HashSet<String>) {
        let mut external_tools: Vec<ToolSpec> = Vec::new();
        let mut external_names: HashSet<String> = HashSet::new();

        if let Ok(ext) = self.store.list_tools(scope, "external").await {
            for t in ext {
                if let Ok(schema) = serde_json::from_str::<serde_json::Value>(&t.schema_json) {
                    let internal_name = format!("user__{}", t.name);
                    external_tools.push(ToolSpec::function(
                        internal_name.clone(),
                        t.summary.clone(),
                        schema,
                    ));
                    external_names.insert(internal_name);
                }
            }
        }

        (external_tools, external_names)
    }

    /// Build the head agent's initial messages using HeadBundleBuilder.
    async fn build_head_messages(
        &self,
        need: &ActiveNeed,
        _scope: &str,
    ) -> Vec<crate::hal::llm::ChatMessage> {
        let bundle_builder = HeadBundleBuilder::new_with_snapshot(
            self.store.clone(),
            self.workspace_root.clone(),
            self.snapshot.clone(),
        );

        let mut scopes = self.scopes.clone();
        if let Some(ref s) = need.scope {
            let s = s.trim();
            if !s.is_empty() {
                let sc = Scope::from(s);
                if !scopes.contains(&sc) {
                    scopes.push(sc);
                }
            }
        }

        let tars = config::load_tars_dials(&self.workspace_root);
        let traits = crate::runtime::AppConfig::global().traits.to_trait_names();
        let bundle_cfg = HeadBundleConfig::new(&self.head_id, scopes)
            .with_context_budget_tokens(config::head_context_budget_tokens())
            .with_time_gap_marker_minutes(config::head_time_gap_marker_minutes())
            .with_traits(traits)
            .with_tars(tars);

        bundle_builder.build(&bundle_cfg).await
    }
}
