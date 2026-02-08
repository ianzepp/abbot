//! Room Runner - Parallel agent execution engine
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Executes a bounded multi-round loop where N agents run independently with
//! private conversation histories, share a common transcript, and synchronize
//! at round boundaries. Each agent runs its own inner tool loop (modeled on
//! MindLoop's dispatch pattern) in parallel within each round.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Parallel-then-sync**: Agents execute concurrently within a round (via
//!   tokio::JoinSet), then synchronize at the round boundary. This maximizes
//!   throughput while maintaining deterministic transcript ordering.
//! - **Signal-based termination**: Three exit conditions — all agents inactive
//!   (noop_done), quiescence (nobody spoke), or round cap hit. No premature
//!   timeouts; agents control their own lifecycle.
//! - **Transcript as shared state**: The only cross-agent communication channel.
//!   Each agent sees what others said, but not their tool calls or internal state.
//!
//! CONCURRENCY
//! ===========
//! - Agents run in parallel per round via `tokio::JoinSet`
//! - Transcript injection happens sequentially between rounds (no contention)
//! - LLM calls go through kernel dispatcher (respects global concurrency limits)

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use uuid::Uuid;

use sqlx::Row;

use crate::hal::llm::{ChatMessage, Role, ToolCall, ToolSpec};
use crate::history::Store;
use crate::kernel::{Frame, FrameOp};
use crate::runtime::Kernel;
use crate::syscalls::dispatch::dispatch_tool;

use super::config::RoomConfig;
use super::tools::build_workspace_context;
use super::types::{AgentRoundResult, Room, TranscriptEntry};
use super::worktree::WorktreeManager;

/// Maximum inner tool loop iterations per agent per round.
///
/// WHY: Safety cap to prevent runaway agents. An agent that makes 20 tool calls
/// in a single round without signaling noop is likely stuck in a loop.
const MAX_INNER_LOOPS: usize = 20;

// =============================================================================
// RUNNER
// =============================================================================

/// Executes parallel multi-agent rounds for a room until quiescence or timeout.
///
/// WHY: Encapsulates the complete room execution lifecycle — worktree provisioning,
/// agent initialization, round execution, summarization, and cleanup. Callers
/// (currently `room:run` syscall) construct a Room and delegate here.
pub struct RoomRunner {
    _store: Arc<Store>,
    /// Room scope string (e.g., "room/issue-42") for frame emission.
    scope: String,
    /// Stable UUID for this room's thread (used as SigcallHub thread_id).
    thread_id: Uuid,
    workspace: PathBuf,
}

impl RoomRunner {
    pub fn new(store: Arc<Store>, scope: &str) -> Self {
        let workspace = Kernel::get()
            .map(|k| k.workspace().to_path_buf())
            .unwrap_or_default();
        Self {
            _store: store,
            scope: scope.to_string(),
            thread_id: Uuid::new_v4(),
            workspace,
        }
    }

    /// Emit a frame through SigcallHub for this room's scope.
    async fn emit_frame(&self, frame: Frame) {
        let Some(k) = Kernel::get() else { return };
        k.sigcalls().send(&self.scope, self.thread_id, frame).await;
    }

    /// Execute the parallel agent loop for a room.
    /// Returns an optional summary string (the room's return value).
    pub async fn run(&self, room: &mut Room, _trace: Option<()>) -> Option<String> {
        // -------------------------------------------------------------------------
        // PHASE 1: WORKTREE PROVISIONING
        // WHY: Work rooms need filesystem isolation so agents can modify code
        // without affecting the main working tree. Provisioned via git worktree.
        // -------------------------------------------------------------------------
        let worktree_path = if room.worktree {
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

        // -------------------------------------------------------------------------
        // PHASE 2: AGENT INITIALIZATION
        // WHY: Each agent needs a system message (identity + room purpose) and
        // initial workspace context before the first round begins.
        // -------------------------------------------------------------------------
        let context = build_workspace_context().await;

        for agent in &mut room.agents {
            let system = include_str!("../../prompts/room/agent_init.md")
                .replace("{system_prompt}", &agent.system_prompt)
                .replace("{prompt}", &room.prompt)
                .replace("{name}", &agent.name)
                .replace("{role}", &agent.role);
            agent.messages.push(ChatMessage::new(Role::System, system));
            agent
                .messages
                .push(ChatMessage::new(Role::User, context.clone()));
        }

        // -------------------------------------------------------------------------
        // PHASE 2b: EMIT ROOM START
        // WHY: Observable room lifecycle — TUI and subscribers see room creation.
        // -------------------------------------------------------------------------
        let agent_names: Vec<&str> = room.agents.iter().map(|a| a.name.as_str()).collect();
        self.emit_frame(
            Frame::event(
                self.thread_id,
                json!({
                    "kind": "room:start",
                    "data": {
                        "room_id": room.id,
                        "room_name": room.name,
                        "prompt": room.prompt,
                        "agent_names": agent_names,
                        "max_rounds": room.max_rounds,
                    }
                }),
            )
            .with_name("room:start")
            .with_actor(format!("room/{}", room.name)),
        )
        .await;

        // Track last polled sequence for user message injection
        let last_polled_seq = Self::current_frame_seq();

        // -------------------------------------------------------------------------
        // PHASE 3: MULTI-ROUND PARALLEL EXECUTION
        // WHY: The core execution loop. Each round fires all active agents in
        // parallel, waits for completion, then processes results before deciding
        // whether to continue.
        // -------------------------------------------------------------------------
        let mut last_polled_seq = last_polled_seq;
        for round in 0..room.max_rounds {
            tracing::debug!(room_id = %room.id, round, "room round start");

            // Inject transcript from previous rounds into each active agent's history
            if round > 0 {
                self.inject_transcript(room);
            }

            // Poll for user messages injected into this room's scope
            last_polled_seq = self.inject_user_messages(room, last_polled_seq).await;

            // Collect active agents for parallel execution
            let active_agents: Vec<(usize, super::types::RoomAgent)> = room
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

            // Fire all active agents in parallel via JoinSet
            let workspace = self.workspace.clone();
            let mut join_set = JoinSet::new();

            for (idx, agent) in active_agents {
                let ws = workspace.clone();
                join_set.spawn(async move {
                    let result = run_agent_round(agent, &ws).await;
                    (idx, result)
                });
            }

            // Synchronization barrier: wait for all agents to complete
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
                agent.messages = output.messages.clone();

                if !output.visible_text.trim().is_empty() {
                    room.transcript.push(TranscriptEntry {
                        agent: agent.name.clone(),
                        content: output.visible_text.clone(),
                        round,
                    });

                    // Emit chat:room frame for each agent that spoke
                    self.emit_frame(
                        Frame::item(
                            self.thread_id,
                            json!({
                                "kind": "chat:room",
                                "data": {
                                    "content": output.visible_text,
                                    "sender": agent.name,
                                    "round": round,
                                }
                            }),
                        )
                        .with_name("chat:message")
                        .with_actor(format!("room/{}", agent.name)),
                    )
                    .await;

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

        // -------------------------------------------------------------------------
        // PHASE 4: SUMMARIZATION
        // WHY: The raw transcript may be verbose. A final LLM call compacts it
        // into a concise summary focused on decisions and action items.
        // -------------------------------------------------------------------------
        let summary = self.summarize(&room.prompt, &room.transcript).await;

        // -------------------------------------------------------------------------
        // PHASE 4b: EMIT ROOM END
        // WHY: Observable room lifecycle — subscribers see room completion.
        // -------------------------------------------------------------------------
        let rounds_completed = room.transcript.iter().map(|t| t.round).max().unwrap_or(0) + 1;
        self.emit_frame(
            Frame::event(
                self.thread_id,
                json!({
                    "kind": "room:end",
                    "data": {
                        "room_id": room.id,
                        "summary": summary,
                        "rounds_completed": rounds_completed,
                    }
                }),
            )
            .with_name("room:end")
            .with_actor(format!("room/{}", room.name)),
        )
        .await;

        // -------------------------------------------------------------------------
        // PHASE 5: CLEANUP
        // WHY: Worktrees consume disk space and git refs. Clean up after execution.
        // -------------------------------------------------------------------------
        self.cleanup_worktree(&room.id, &worktree_path);

        summary
    }

    /// Inject new shared transcript entries into each active agent's private history.
    ///
    /// WHY: This is the cross-agent communication mechanism. Each agent sees what
    /// others said in the previous round (but not their own messages, which are
    /// already in their private history as assistant messages).
    fn inject_transcript(&self, room: &mut Room) {
        let latest_round = room.transcript.iter().map(|t| t.round).max().unwrap_or(0);

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
                    continue; // WHY: Skip own messages — already in private history
                }
                if !transcript_text.is_empty() {
                    transcript_text.push_str("\n\n");
                }
                transcript_text.push_str(&format!("[{}]: {}", entry.agent, entry.content));
            }

            if !transcript_text.is_empty() {
                agent.messages.push(ChatMessage::new(
                    Role::User,
                    format!(
                        "## Discussion (Round {})\n\n{}",
                        latest_round, transcript_text
                    ),
                ));
            }
        }
    }

    /// Summarize the transcript with a final LLM call.
    ///
    /// WHY: Raw transcripts can be long and repetitive. The summarizer extracts
    /// decisions, action items, and key insights into a concise return value.
    ///
    /// TRADE-OFF: If LLM is unavailable, falls back to raw transcript concatenation
    /// rather than returning None — some output is better than no output.
    async fn summarize(&self, prompt: &str, transcript: &[TranscriptEntry]) -> Option<String> {
        if transcript.is_empty() {
            return None;
        }

        let room_cfg = RoomConfig::from_config();
        if !room_cfg.llm.enabled {
            // WHY: Fallback to raw transcript when LLM not available
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
                include_str!("../../prompts/room/summarizer.md").trim(),
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

    /// Poll FrameStore for user messages injected into this room's scope since last_seq.
    /// Returns the new last_polled_seq for the next round.
    async fn inject_user_messages(&self, room: &mut Room, last_seq: i64) -> i64 {
        let Some(k) = Kernel::get() else {
            return last_seq;
        };
        let Some(store) = k.frames() else {
            return last_seq;
        };
        let pool = store.pool();

        let rows = sqlx::query(
            "SELECT seq, frame_json FROM frames \
             WHERE seq > ? AND scope = ? AND kind = 'chat:user' \
             ORDER BY seq ASC LIMIT 50",
        )
        .bind(last_seq)
        .bind(&self.scope)
        .fetch_all(pool)
        .await;

        let rows = match rows {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "failed to poll user messages for room");
                return last_seq;
            }
        };

        let mut new_seq = last_seq;
        for row in &rows {
            let seq: i64 = row.get(0);
            let frame_json: String = row.get(1);
            new_seq = new_seq.max(seq);

            // Extract content from the frame JSON
            let content = serde_json::from_str::<serde_json::Value>(&frame_json)
                .ok()
                .and_then(|v| {
                    v.get("data")
                        .and_then(|d| d.get("data"))
                        .and_then(|d| d.get("content"))
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            v.get("data")
                                .and_then(|d| d.get("content"))
                                .and_then(|c| c.as_str())
                                .map(|s| s.to_string())
                        })
                });

            if let Some(content) = content
                && !content.trim().is_empty()
            {
                // Inject into all active agents' message histories
                for agent in &mut room.agents {
                    if agent.active {
                        agent
                            .messages
                            .push(ChatMessage::new(Role::User, format!("[User]: {}", content)));
                    }
                }
                tracing::info!(scope = %self.scope, "injected user message into room agents");
            }
        }

        new_seq
    }

    /// Get current max sequence from FrameStore (for tracking injection cursor).
    fn current_frame_seq() -> i64 {
        Kernel::get()
            .and_then(|k| k.frames())
            .map(|s| s.last_seq() as i64)
            .unwrap_or(0)
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
//
// Each agent runs an inner tool loop within a single round: call LLM → check
// for noop signals → dispatch tool calls → repeat until signal or cap hit.
// This runs on a spawned tokio task for parallel execution across agents.

/// Output from a single agent's round execution.
struct AgentRoundOutput {
    /// Updated private message history.
    messages: Vec<ChatMessage>,
    /// Visible text produced this round (outside `<thinking>` tags).
    visible_text: String,
    /// Round result classification.
    result: AgentRoundResult,
}

/// Run a single agent's inner tool loop for one round.
///
/// WHY separate function: Each agent runs on its own tokio task. This function
/// owns the agent's mutable state for the duration of the round, then returns
/// the updated state for the runner to merge back.
async fn run_agent_round(mut agent: super::types::RoomAgent, workspace: &Path) -> AgentRoundOutput {
    let actor = format!("room/{}", agent.name);
    let mut visible_text = String::new();
    let mut result = AgentRoundResult::Spoke;
    let mut vfs_cwd = String::from("/");

    for _iteration in 0..MAX_INNER_LOOPS {
        let llm_result = match call_llm(&agent.messages, &agent.tools, &actor, workspace).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(agent = %agent.name, error = %e, "agent LLM call failed");
                result = AgentRoundResult::Signal;
                break;
            }
        };

        // Collect visible text (excluding <thinking> tags)
        if let Some(ref content) = llm_result.content {
            let cleaned = strip_thinking_tags(content);
            if !cleaned.trim().is_empty() {
                if !visible_text.is_empty() {
                    visible_text.push(' ');
                }
                visible_text.push_str(cleaned.trim());
            }
        }

        // No tool calls = agent is done speaking naturally
        if llm_result.tool_calls.is_empty() {
            break;
        }

        // Check for terminal signals before dispatching tools
        let has_done = llm_result
            .tool_calls
            .iter()
            .any(|tc| tc.function.name == "tool__noop_done");

        if has_done {
            result = AgentRoundResult::Done;
            break;
        }

        let has_signal = llm_result
            .tool_calls
            .iter()
            .any(|tc| tc.function.name == "tool__noop_signal");

        if has_signal {
            result = AgentRoundResult::Signal;
            break;
        }

        // Dispatch non-noop tool calls through kernel
        agent.messages.push(ChatMessage::assistant_tool_calls(
            llm_result.tool_calls.clone(),
        ));

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
                &vfs_cwd,
            )
            .await;

            // WHY track VFS CWD: fs:cd changes the agent's working directory,
            // and subsequent fs operations need the updated path.
            if tc.function.name == "tool__fs_cd"
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(&out)
                && v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false)
                && let Some(new_cwd) = v
                    .get("data")
                    .and_then(|d| d.get("cwd"))
                    .and_then(|c| c.as_str())
            {
                vfs_cwd = new_cwd.to_string();
            }

            agent
                .messages
                .push(ChatMessage::tool_result(tc.id.clone(), out));
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
//
// Two LLM calling patterns: full (with tools, for agent rounds) and simple
// (without tools, for summarization). Both route through the kernel dispatcher
// via the `llm:chat` syscall.

/// Streamed LLM response containing text content and/or tool calls.
struct LlmResult {
    content: Option<String>,
    tool_calls: Vec<ToolCall>,
}

/// Call the LLM via llm:chat syscall and collect the streamed response.
///
/// WHY route through dispatcher: Respects global concurrency limits, rate
/// limiting, and provider configuration. The agent doesn't need to know
/// which LLM provider is configured.
async fn call_llm(
    messages: &[ChatMessage],
    tools: &[ToolSpec],
    actor: &str,
    workspace: &Path,
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
    let mut rx = dispatcher.dispatch(req, workspace.to_path_buf(), CancellationToken::new());

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
                        let arguments_v =
                            data.get("arguments").cloned().unwrap_or_else(|| json!({}));
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
///
/// WHY separate from `call_llm`: Summarization doesn't need tool support,
/// and using a distinct actor name ("system/room_summarizer") makes frame
/// logs easier to filter.
async fn call_llm_simple(messages: &[ChatMessage], workspace: &Path) -> Result<String, String> {
    let Some(k) = Kernel::get() else {
        return Err("kernel not initialized".to_string());
    };
    let dispatcher = k.dispatcher().await;

    let payload = json!({ "messages": messages });
    let req = Frame::req("llm:chat", payload).with_actor("system/room_summarizer");
    let mut rx = dispatcher.dispatch(req, workspace.to_path_buf(), CancellationToken::new());

    let mut content = String::new();
    while let Some(frame) = rx.recv().await {
        match frame.op {
            FrameOp::Item => {
                if let Some(data) = frame.data
                    && data.get("type").and_then(|v| v.as_str()) == Some("text_delta")
                    && let Some(text) = data.get("content").and_then(|v| v.as_str())
                {
                    content.push_str(text);
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

// =============================================================================
// TEXT HELPERS
// =============================================================================

/// Strip `<thinking>...</thinking>` tags from content.
///
/// WHY: Some models emit reasoning in thinking tags. These should not appear
/// in the shared transcript since they're internal agent reasoning, not
/// visible communication.
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
