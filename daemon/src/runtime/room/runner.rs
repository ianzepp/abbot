//! Room Runner - Parallel agent execution engine
//!
//! Executes a bounded multi-round loop where N agents run independently with
//! private conversation histories, share a common transcript, and synchronize
//! at round boundaries. Each agent runs its own inner tool loop (modeled on
//! MindLoop) in parallel within each round.
//!
//! Round lifecycle:
//! 1. Inject new shared transcript into each active agent's private history
//! 2. Fire all active agents in parallel (tokio::JoinSet)
//! 3. Wait for all agents (synchronization barrier)
//! 4. Append visible text to shared transcript
//! 5. Deactivate noop_done agents
//! 6. Terminate if: all agents inactive, no new chat, or round cap hit
//! 7. Final summarizer LLM call compacts transcript into return value

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::history::Store;
use crate::kernel::{Frame, FrameOp};
use crate::hal::llm::{ChatMessage, Role, ToolCall, ToolSpec};
use crate::runtime::Kernel;
use crate::scope::Scope;
use crate::syscalls::dispatch::dispatch_tool;

use super::bundle::{RoomBundleBuilder, RoomBundleConfig, WakeMode};
use super::config::RoomConfig;
use super::types::{AgentRoundResult, Room, RoomAgent, RoomType, TranscriptEntry};
use super::worktree::WorktreeManager;

/// Maximum inner tool loop iterations per agent per round.
const MAX_INNER_LOOPS: usize = 20;

// =============================================================================
// RUNNER
// =============================================================================

/// Executes parallel multi-agent rounds for a room until quiescence or timeout.
pub struct RoomRunner {
    store: Arc<Store>,
    scopes: Vec<Scope>,
    workspace: PathBuf,
    config: RoomConfig,
}

impl RoomRunner {
    pub fn new(
        store: Arc<Store>,
        scopes: Vec<Scope>,
        workspace: PathBuf,
        config: RoomConfig,
    ) -> Self {
        Self {
            store,
            scopes,
            workspace,
            config,
        }
    }

    /// Execute the parallel agent loop for a room.
    /// Returns an optional summary string (the room's return value).
    pub async fn run(
        &self,
        room: &mut Room,
        _trace: Option<()>,
    ) -> Option<String> {
        // Phase 1: Worktree provisioning for Work rooms
        let worktree_path = if room.room_type == RoomType::Work {
            let mgr = WorktreeManager::new(&self.workspace);
            match mgr.provision(&room.id, None) {
                Ok(path) => {
                    tracing::info!(room_id = %room.id, path = %path.display(), "provisioned worktree");
                    Some(path)
                }
                Err(e) => {
                    tracing::error!(room_id = %room.id, error = %e, "failed to provision worktree");
                    return None;
                }
            }
        } else {
            None
        };

        // Phase 2: Build context and initialize agent messages
        let context = self.build_context(room.room_type);

        for agent in &mut room.agents {
            // System message: agent's system prompt + room purpose
            let system = format!(
                "{}\n\n## Room Purpose\n\n{}\n\n## Your Role\n\nYou are {} ({}). \
                 Use tool__noop_signal when you are done for this round. \
                 Use tool__noop_done when you have nothing more to contribute and want to leave permanently.",
                agent.system_prompt, room.prompt, agent.name, agent.role
            );
            agent.messages.push(ChatMessage::new(Role::System, system));

            // Initial user context
            agent.messages.push(ChatMessage::new(Role::User, context.clone()));
        }

        // Phase 3: Multi-round parallel execution
        for round in 0..room.max_rounds {
            tracing::debug!(room_id = %room.id, round, "room round start");

            // Inject transcript from previous rounds into each active agent's history
            if round > 0 {
                self.inject_transcript(room);
            }

            // Fire all active agents in parallel
            let active_agents: Vec<(usize, RoomAgent)> = room
                .agents
                .iter()
                .enumerate()
                .filter(|(_, a)| a.active)
                .map(|(i, a)| (i, a.clone()))
                .collect();

            if active_agents.is_empty() {
                tracing::debug!(room_id = %room.id, "all agents inactive, ending room");
                break;
            }

            let workspace = self.workspace.clone();
            let mut join_set = JoinSet::new();

            for (idx, agent) in active_agents {
                let ws = workspace.clone();
                join_set.spawn(async move {
                    let result = run_agent_round(agent, &ws).await;
                    (idx, result)
                });
            }

            // Synchronization barrier: wait for all agents
            let mut round_results: Vec<(usize, AgentRoundOutput)> = Vec::new();
            while let Some(result) = join_set.join_next().await {
                match result {
                    Ok((idx, output)) => round_results.push((idx, output)),
                    Err(e) => tracing::error!(error = %e, "agent task panicked"),
                }
            }

            // Process results: update agents, append transcript, deactivate done agents
            let mut anyone_spoke = false;

            for (idx, output) in &round_results {
                let agent = &mut room.agents[*idx];

                // Update agent's private message history from the output
                agent.messages = output.messages.clone();

                // Append visible text to shared transcript
                if !output.visible_text.trim().is_empty() {
                    room.transcript.push(TranscriptEntry {
                        agent: agent.name.clone(),
                        content: output.visible_text.clone(),
                        round,
                    });
                    anyone_spoke = true;
                }

                match output.result {
                    AgentRoundResult::Done => {
                        agent.active = false;
                        tracing::info!(agent = %agent.name, round, "agent left room (noop_done)");
                    }
                    AgentRoundResult::Signal => {
                        tracing::debug!(agent = %agent.name, round, "agent signaled (noop_signal)");
                    }
                    AgentRoundResult::Spoke => {
                        tracing::debug!(agent = %agent.name, round, "agent spoke");
                    }
                }
            }

            // Check termination conditions
            let all_inactive = room.agents.iter().all(|a| !a.active);
            if all_inactive {
                tracing::debug!(room_id = %room.id, round, "all agents left, ending room");
                break;
            }

            if !anyone_spoke {
                tracing::debug!(room_id = %room.id, round, "quiescence (nobody spoke), ending room");
                break;
            }
        }

        // Phase 4: Summarize transcript
        let summary = self.summarize(&room.prompt, &room.transcript).await;

        // Phase 5: Persist and cleanup
        let transcript_json = serde_json::to_string(&room.transcript).unwrap_or_default();
        let summary_json = summary
            .as_ref()
            .map(|s| json!({"summary": s}).to_string())
            .unwrap_or_else(|| "{}".to_string());
        if let Err(e) = self.store.save_conclave(&room.id, "done", &transcript_json, &summary_json) {
            tracing::error!(error = %e, "failed to save room");
        }

        self.cleanup_worktree(&room.id, &worktree_path);

        summary
    }

    /// Inject new shared transcript entries into each active agent's private history.
    /// Skip entries from the agent itself (already in their history as assistant messages).
    fn inject_transcript(&self, room: &mut Room) {
        // Find the latest round in the transcript
        let latest_round = room.transcript.iter().map(|t| t.round).max().unwrap_or(0);

        // Get entries from the latest round only
        let new_entries: Vec<&TranscriptEntry> = room
            .transcript
            .iter()
            .filter(|t| t.round == latest_round)
            .collect();

        if new_entries.is_empty() {
            return;
        }

        for agent in &mut room.agents {
            if !agent.active {
                continue;
            }

            let mut transcript_text = String::new();
            for entry in &new_entries {
                if entry.agent == agent.name {
                    continue; // Skip own messages
                }
                if !transcript_text.is_empty() {
                    transcript_text.push_str("\n\n");
                }
                transcript_text.push_str(&format!("[{}]: {}", entry.agent, entry.content));
            }

            if !transcript_text.is_empty() {
                agent.messages.push(ChatMessage::new(
                    Role::User,
                    format!("## Discussion (Round {})\n\n{}", latest_round, transcript_text),
                ));
            }
        }
    }

    /// Build combined context from bundle builder.
    fn build_context(&self, room_type: RoomType) -> String {
        let bundle_builder = RoomBundleBuilder::new(self.store.clone());
        let bundle_type = match room_type {
            RoomType::Conclave => super::bundle::RoomType::Conclave,
            RoomType::Autonomy => super::bundle::RoomType::Autonomy,
            RoomType::Work => super::bundle::RoomType::Work,
        };
        let bundle_cfg = RoomBundleConfig::new("conclave", self.scopes.clone())
            .with_wake_mode(WakeMode::Normal)
            .with_workspace(self.workspace.clone())
            .with_fever(self.config.fever.clone())
            .with_filter(self.config.filter.clone())
            .with_poverty(self.config.poverty.clone())
            .with_room_type(bundle_type);
        let messages = bundle_builder.build(&bundle_cfg);

        let mut parts = Vec::new();
        if let Some(system) = messages.iter().find(|m| matches!(m.role, Role::System)) {
            if let Some(content) = &system.content {
                parts.push(content.clone());
            }
        }
        if let Some(user) = messages.iter().find(|m| matches!(m.role, Role::User)) {
            if let Some(content) = &user.content {
                parts.push(content.clone());
            }
        }
        parts.join("\n\n")
    }

    /// Summarize the transcript with a final LLM call.
    async fn summarize(&self, prompt: &str, transcript: &[TranscriptEntry]) -> Option<String> {
        if transcript.is_empty() {
            return None;
        }

        let room_cfg = RoomConfig::from_config();
        if !room_cfg.llm.enabled {
            // Return raw transcript if LLM not available
            let text = transcript
                .iter()
                .map(|t| format!("[{}]: {}", t.agent, t.content))
                .collect::<Vec<_>>()
                .join("\n");
            return Some(text);
        }

        let transcript_text = transcript
            .iter()
            .map(|t| format!("[{} (round {})]: {}", t.agent, t.round, t.content))
            .collect::<Vec<_>>()
            .join("\n\n");

        let messages = vec![
            ChatMessage::new(
                Role::System,
                "You are a concise summarizer. Produce a brief summary of the room discussion. \
                 Focus on decisions made, actions agreed upon, and key insights. Keep it under 500 words.",
            ),
            ChatMessage::new(
                Role::User,
                format!(
                    "## Room Purpose\n\n{}\n\n## Discussion Transcript\n\n{}\n\nSummarize the key outcomes.",
                    prompt, transcript_text
                ),
            ),
        ];

        match call_llm_simple(&messages, &self.workspace).await {
            Ok(content) => {
                if content.trim().is_empty() {
                    None
                } else {
                    Some(content)
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to summarize room");
                // Fallback: return raw transcript
                Some(
                    transcript
                        .iter()
                        .map(|t| format!("[{}]: {}", t.agent, t.content))
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }
        }
    }

    fn cleanup_worktree(&self, room_id: &str, worktree_path: &Option<PathBuf>) {
        if worktree_path.is_some() {
            let mgr = WorktreeManager::new(&self.workspace);
            if let Err(e) = mgr.cleanup(room_id) {
                tracing::warn!(room_id = %room_id, error = %e, "failed to cleanup worktree");
            }
        }
    }
}

// =============================================================================
// PER-AGENT ROUND EXECUTION
// =============================================================================

/// Output from a single agent's round execution.
struct AgentRoundOutput {
    /// Updated private message history.
    messages: Vec<ChatMessage>,
    /// Visible text produced this round (outside <thinking> tags).
    visible_text: String,
    /// Round result classification.
    result: AgentRoundResult,
}

/// Run a single agent's inner tool loop for one round.
/// Modeled on MindLoop's dispatch pattern.
async fn run_agent_round(
    mut agent: RoomAgent,
    workspace: &PathBuf,
) -> AgentRoundOutput {
    let actor = format!("room/{}", agent.name);
    let mut visible_text = String::new();
    let mut result = AgentRoundResult::Spoke;

    for _iteration in 0..MAX_INNER_LOOPS {
        // Call LLM with agent's messages and tools
        let llm_result = match call_llm(&agent.messages, &agent.tools, &actor, workspace).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(agent = %agent.name, error = %e, "agent LLM call failed");
                result = AgentRoundResult::Signal;
                break;
            }
        };

        // Collect visible text (text_delta content, excluding <thinking> tags)
        if let Some(ref content) = llm_result.content {
            let cleaned = strip_thinking_tags(content);
            if !cleaned.trim().is_empty() {
                if !visible_text.is_empty() {
                    visible_text.push(' ');
                }
                visible_text.push_str(cleaned.trim());
            }
        }

        // No tool calls = agent is done speaking
        if llm_result.tool_calls.is_empty() {
            break;
        }

        // Check for noop_done
        let has_done = llm_result
            .tool_calls
            .iter()
            .any(|tc| tc.function.name == "tool__noop_done");

        if has_done {
            result = AgentRoundResult::Done;
            break;
        }

        // Check for noop_signal
        let has_signal = llm_result
            .tool_calls
            .iter()
            .any(|tc| tc.function.name == "tool__noop_signal");

        if has_signal {
            result = AgentRoundResult::Signal;
            break;
        }

        // Dispatch non-noop tool calls
        agent
            .messages
            .push(ChatMessage::assistant_tool_calls(llm_result.tool_calls.clone()));

        for tc in &llm_result.tool_calls {
            tracing::debug!(
                agent = %agent.name,
                tool = %tc.function.name,
                "room agent dispatching tool"
            );

            let out = dispatch_tool(
                &tc.function.name,
                &tc.function.arguments,
                &actor,
                workspace,
            )
            .await;

            agent.messages.push(ChatMessage::tool_result(tc.id.clone(), out));
        }
    }

    AgentRoundOutput {
        messages: agent.messages,
        visible_text,
        result,
    }
}

// =============================================================================
// LLM HELPERS
// =============================================================================

struct LlmResult {
    content: Option<String>,
    tool_calls: Vec<ToolCall>,
}

/// Call the LLM via llm:chat syscall and collect streamed response.
/// Modeled on MindLoop's call_llm.
async fn call_llm(
    messages: &[ChatMessage],
    tools: &[ToolSpec],
    actor: &str,
    workspace: &PathBuf,
) -> Result<LlmResult, String> {
    let Some(k) = Kernel::get() else {
        return Err("kernel not initialized".to_string());
    };
    let dispatcher = k.dispatcher().await;

    let payload = json!({
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
    });

    let req = Frame::req("llm:chat", payload).with_actor(actor.to_string());
    let mut rx = dispatcher.dispatch(req, workspace.clone(), CancellationToken::new());

    let mut content = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    while let Some(frame) = rx.recv().await {
        match frame.op {
            FrameOp::Item => {
                let Some(data) = frame.data else { continue };
                match data.get("type").and_then(|v| v.as_str()) {
                    Some("text_delta") => {
                        if let Some(text) = data.get("content").and_then(|v| v.as_str()) {
                            content.push_str(text);
                        }
                    }
                    Some("tool_call") => {
                        let id = data
                            .get("tool_call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = data
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments_v = data
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| json!({}));
                        let arguments = serde_json::to_string(&arguments_v)
                            .ok()
                            .filter(|s| s.trim_start().starts_with('{'))
                            .unwrap_or_else(|| "{}".to_string());
                        if !id.is_empty() && !name.is_empty() {
                            let value = json!({
                                "id": id,
                                "type": "function",
                                "function": {"name": name, "arguments": arguments}
                            });
                            if let Ok(tc) = serde_json::from_value::<ToolCall>(value) {
                                tool_calls.push(tc);
                            }
                        }
                    }
                    _ => {}
                }
            }
            FrameOp::Error => {
                let msg = frame
                    .data
                    .as_ref()
                    .and_then(|d| d.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("llm error");
                return Err(msg.to_string());
            }
            FrameOp::Done => break,
            _ => {}
        }
    }

    Ok(LlmResult {
        content: if content.is_empty() {
            None
        } else {
            Some(content)
        },
        tool_calls,
    })
}

/// Simple LLM call without tools (for summarization).
async fn call_llm_simple(
    messages: &[ChatMessage],
    workspace: &PathBuf,
) -> Result<String, String> {
    let Some(k) = Kernel::get() else {
        return Err("kernel not initialized".to_string());
    };
    let dispatcher = k.dispatcher().await;

    let payload = json!({ "messages": messages });
    let req = Frame::req("llm:chat", payload).with_actor("system/room_summarizer");
    let mut rx = dispatcher.dispatch(req, workspace.clone(), CancellationToken::new());

    let mut content = String::new();
    while let Some(frame) = rx.recv().await {
        match frame.op {
            FrameOp::Item => {
                if let Some(data) = frame.data {
                    if data.get("type").and_then(|v| v.as_str()) == Some("text_delta") {
                        if let Some(text) = data.get("content").and_then(|v| v.as_str()) {
                            content.push_str(text);
                        }
                    }
                }
            }
            FrameOp::Error => {
                let msg = frame
                    .data
                    .as_ref()
                    .and_then(|d| d.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("llm error");
                return Err(msg.to_string());
            }
            FrameOp::Done => break,
            _ => {}
        }
    }

    Ok(content)
}

/// Strip `<thinking>...</thinking>` tags from content.
fn strip_thinking_tags(content: &str) -> String {
    let mut result = content.to_string();
    while let Some(start) = result.find("<thinking>") {
        if let Some(end) = result.find("</thinking>") {
            let end = end + "</thinking>".len();
            result.replace_range(start..end, "");
        } else {
            // Unclosed thinking tag — strip from start to end
            result = result[..start].to_string();
            break;
        }
    }
    result
}
