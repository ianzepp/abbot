//! Room Tools - Tool specifications and deliberation tracking
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Defines two tool sets for room participants and the ProposalTracker that
//! collects proposals and votes during deliberation:
//! - Coordination tools (propose, vote, done): Used in conclave/autonomy rooms
//!   for structured multi-agent consensus.
//! - Work tools (explore, edit, test, commit, shell): Used in work rooms for
//!   code changes in isolated worktrees.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Tool-based interaction: Participants call structured tools rather than
//!   emitting free-form JSON. This enables validation, tracking, and phased
//!   execution (readonly tools can run in parallel).
//! - 2/3 vote threshold: Proposals require at least 2 yes votes to pass.
//!   With 3 participants, this means at least 2 must agree. The proposer's
//!   implicit yes vote counts toward the threshold.
//!
//! TRADE-OFFS
//! ==========
//! - Work tools dispatch to kernel syscalls (fs:read, fs:write, proc:run,
//!   git:run) rather than executing directly. This adds latency but ensures
//!   all operations go through the kernel's permission and logging systems.
//! - The 2/3 threshold is hardcoded. With exactly 3 participants this works
//!   well, but would need adjustment for different participant counts.

use serde_json::{json, Value};

use crate::hal::llm::ToolSpec;

use super::types::{
    RoomDecision, NeedProposal, WantProposal, LtmProposal, SelfProposal, ControlProposal,
};


// =============================================================================
// COORDINATION TOOLS (conclave/autonomy rooms)
// =============================================================================
//
// WHY three tools: propose/vote/done maps to the deliberation lifecycle.
// Participants propose actions, vote on each other's proposals, and signal
// consensus when they believe all proposals are resolved.

/// Build tool specs for coordination rooms (conclave/autonomy).
pub fn coordination_tool_specs() -> Vec<ToolSpec> {
    crate::tool_specs![
        "room__propose",
        "room__vote",
        "room__done",
    ]
}

// =============================================================================
// WORK TOOLS (work rooms)
// =============================================================================
//
// WHY separate tool set: Work rooms operate on code in isolated worktrees.
// These tools map to kernel syscalls (fs:read, fs:write, proc:run, git:run)
// with paths scoped to the worktree directory.

/// Build tool specs for work rooms (code execution).
pub fn work_tool_specs() -> Vec<ToolSpec> {
    crate::tool_specs![
        "hand__explore",
        "hand__edit",
        "hand__test",
        "hand__commit",
        "hand__shell",
    ]
}

// =============================================================================
// TOOL CLASSIFICATION
// =============================================================================
//
// WHY classify: Readonly tools can be executed in parallel during a round,
// while mutating tools must be serialized to prevent conflicts.

/// Classify whether a tool is read-only (safe for parallel execution).
pub fn is_readonly(name: &str) -> bool {
    matches!(name, "room__vote" | "room__done" | "hand__explore")
}

// =============================================================================
// PROPOSAL TRACKER
// =============================================================================
//
// WHY a dedicated tracker: Centralizes proposal collection, vote tallying,
// and consensus detection. Decouples the runner loop from proposal semantics
// so the runner only needs to feed parsed data into the tracker.

/// Track proposals, votes, and done signals during room deliberation.
///
/// WHY this exists: The runner feeds participant responses into the tracker,
/// which maintains state across rounds and produces the final RoomDecision
/// via tally_decision().
///
/// INVARIANTS:
/// ----------
/// INV-1: A proposer implicitly votes "yes" on their own proposal.
/// INV-2: Proposals require >= 2 yes votes to be included in the decision.
#[derive(Debug, Default)]
pub struct ProposalTracker {
    /// (proposer_name, proposal) pairs in submission order.
    pub proposals: Vec<(String, TrackedProposal)>,
    /// proposal_key -> {voter_name -> vote}. Key format: "type:text".
    pub votes: std::collections::HashMap<String, std::collections::HashMap<String, String>>,
    /// Participants who have called room__done.
    pub done_signals: std::collections::HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct TrackedProposal {
    pub proposal_type: String,
    pub text: String,
    pub context: String,
    pub priority: String,
    pub content: String,
    pub pattern: String,
    pub mode: String,
}

impl ProposalTracker {
    /// Record a new proposal and implicit yes vote from the proposer.
    pub fn handle_propose(&mut self, args: &Value, caller: &str) -> String {
        let proposal_type = args.get("proposal_type").and_then(|v| v.as_str()).unwrap_or("");
        let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let context = args.get("context").and_then(|v| v.as_str()).unwrap_or("");
        let priority = args.get("priority").and_then(|v| v.as_str()).unwrap_or("normal");
        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("");

        let proposal = TrackedProposal {
            proposal_type: proposal_type.to_string(),
            text: text.to_string(),
            context: context.to_string(),
            priority: priority.to_string(),
            content: content.to_string(),
            pattern: pattern.to_string(),
            mode: mode.to_string(),
        };

        // WHY implicit yes: The proposer is assumed to support their own proposal.
        // This ensures a proposal from any participant needs only one additional vote.
        let key = format!("{}:{}", proposal_type, text);
        self.votes
            .entry(key.clone())
            .or_default()
            .insert(caller.to_string(), "yes".to_string());

        self.proposals.push((caller.to_string(), proposal));

        format!("Proposal recorded: [{}] {}. Implicit yes vote from {}.", proposal_type, text, caller)
    }

    /// Record a vote on an existing proposal.
    pub fn handle_vote(&mut self, args: &Value, caller: &str) -> String {
        let key = args.get("proposal_key").and_then(|v| v.as_str()).unwrap_or("");
        let vote = args.get("vote").and_then(|v| v.as_str()).unwrap_or("abstain");

        self.votes
            .entry(key.to_string())
            .or_default()
            .insert(caller.to_string(), vote.to_string());

        format!("Vote recorded: {} votes {} on {}", caller, vote, key)
    }

    /// Record a done signal from a participant.
    pub fn handle_done(&mut self, args: &Value, caller: &str) -> String {
        let thoughts = args.get("thoughts").and_then(|v| v.as_str()).unwrap_or("");
        self.done_signals.insert(caller.to_string());
        format!("{} signals done. {}", caller, thoughts)
    }

    /// Check if all participants have signaled done.
    pub fn all_done(&self, participant_names: &[String]) -> bool {
        participant_names.iter().all(|name| self.done_signals.contains(name))
    }

    /// Tally votes and build a RoomDecision from approved proposals.
    ///
    /// WHY 2/3 threshold: With 3 participants, requiring 2 yes votes ensures
    /// majority agreement while allowing one dissenter. The proposer's implicit
    /// yes means any proposal with one additional supporter passes.
    pub fn tally_decision(&self) -> RoomDecision {
        let mut decision = RoomDecision::default();

        for (proposer, p) in &self.proposals {
            let key = format!("{}:{}", p.proposal_type, p.text);
            let vote_map = self.votes.get(&key);

            let yes_count = vote_map
                .map(|v| v.values().filter(|&vote| vote == "yes").count())
                .unwrap_or(0);

            // WHY >= 2: With 3 participants and implicit yes from proposer,
            // this requires at least one other participant to agree.
            if yes_count >= 2 {
                match p.proposal_type.as_str() {
                    "need" => {
                        let priority = if p.priority.is_empty() { "normal" } else { &p.priority };
                        // WHY reconvene on urgent: Urgent needs should be followed up
                        // immediately rather than waiting for the next idle cycle.
                        let reconvene = priority == "urgent";
                        decision.needs.push(NeedProposal {
                            need: p.text.clone(),
                            context: p.context.clone(),
                            priority: priority.to_string(),
                            reconvene,
                            votes: vote_map
                                .map(|v| {
                                    v.iter()
                                        .filter(|(_, vote)| *vote == "yes")
                                        .map(|(m, _)| m.clone())
                                        .collect()
                                })
                                .unwrap_or_default(),
                        });
                    }
                    "want" => {
                        decision.wants.push(WantProposal {
                            want: p.text.clone(),
                            context: p.context.clone(),
                            priority: if p.priority.is_empty() {
                                "normal".to_string()
                            } else {
                                p.priority.clone()
                            },
                            proposer: proposer.clone(),
                        });
                    }
                    "ltm" => {
                        decision.ltm_ops.push(LtmProposal {
                            kind: p.text.clone(),
                            content: p.content.clone(),
                            pattern: p.pattern.clone(),
                            proposer: proposer.clone(),
                        });
                    }
                    "self" => {
                        decision.self_ops.push(SelfProposal {
                            kind: p.text.clone(),
                            content: p.content.clone(),
                            pattern: p.pattern.clone(),
                            proposer: proposer.clone(),
                        });
                    }
                    "control" => {
                        // WHY mode fallback: If mode is empty, use priority as mode
                        // (legacy compat), defaulting to "hard" if both are empty.
                        let mode = if p.mode.is_empty() {
                            if !p.priority.is_empty() {
                                p.priority.clone()
                            } else {
                                "hard".to_string()
                            }
                        } else {
                            p.mode.clone()
                        };

                        decision.control_ops.push(ControlProposal {
                            kind: p.text.clone(),
                            mode,
                            reason: p.context.clone(),
                            proposer: proposer.clone(),
                        });
                    }
                    _ => {}
                }
            }
        }

        decision
    }
}

// =============================================================================
// WORK TOOL DISPATCH
// =============================================================================
//
// WHY dispatch through kernel: Work tools could execute directly, but routing
// through kernel syscalls ensures permission checks, logging, and VFS
// enforcement are applied consistently.

/// Execute a work tool via kernel syscalls.
///
/// WHY worktree_path parameter: All file operations are scoped to the work
/// room's isolated worktree directory, preventing accidental modification of
/// the main working tree.
///
/// SECURITY NOTE: Paths are joined with worktree_path to constrain operations.
/// However, path traversal (../) is not validated here — the underlying fs:read
/// and fs:write syscalls enforce VFS boundaries.
pub async fn execute_work_tool(
    name: &str,
    args: &Value,
    worktree_path: &std::path::Path,
) -> String {
    let Some(k) = crate::runtime::Kernel::get() else {
        return "error: kernel not initialized".to_string();
    };

    let dispatcher = k.dispatcher().await;

    match name {
        // WHY fs:read for explore: Reuses the existing VFS-aware read syscall
        // rather than implementing a separate file reader.
        "hand__explore" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let full_path = worktree_path.join(path);
            let req = crate::kernel::Frame::req(
                "fs:read",
                json!({"path": full_path.to_string_lossy()}),
            )
            .with_actor("system/room_worker");

            collect_dispatch_output(&dispatcher, req, worktree_path).await
        }
        "hand__edit" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let full_path = worktree_path.join(path);
            let req = crate::kernel::Frame::req(
                "fs:write",
                json!({"path": full_path.to_string_lossy(), "content": content}),
            )
            .with_actor("system/room_worker");

            collect_dispatch_output(&dispatcher, req, worktree_path).await
        }
        // WHY combined: test and shell both execute commands via proc:run.
        // The distinction is semantic (for the LLM) not functional.
        "hand__test" | "hand__shell" => {
            let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let req = crate::kernel::Frame::req(
                "proc:run",
                json!({"command": command, "cwd": worktree_path.to_string_lossy()}),
            )
            .with_actor("system/room_worker");

            collect_dispatch_output(&dispatcher, req, worktree_path).await
        }
        // WHY -am: Stages and commits all changes in a single operation.
        // Work rooms should make focused changes so staging all is appropriate.
        "hand__commit" => {
            let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("room commit");
            let req = crate::kernel::Frame::req(
                "git:run",
                json!({"args": ["commit", "-am", message], "cwd": worktree_path.to_string_lossy()}),
            )
            .with_actor("system/room_worker");

            collect_dispatch_output(&dispatcher, req, worktree_path).await
        }
        _ => format!("unknown work tool: {}", name),
    }
}

/// Collect output from a dispatched syscall frame stream.
///
/// WHY: Syscalls emit multiple frames (items, ok, error, done). This helper
/// concatenates content/stdout fields into a single string result.
async fn collect_dispatch_output(
    dispatcher: &crate::kernel::KernelDispatcher,
    req: crate::kernel::Frame,
    workspace: &std::path::Path,
) -> String {
    let mut rx = dispatcher.dispatch(
        req,
        workspace.to_path_buf(),
        tokio_util::sync::CancellationToken::new(),
    );

    let mut output = String::new();
    while let Some(frame) = rx.recv().await {
        match frame.op {
            crate::kernel::FrameOp::Ok | crate::kernel::FrameOp::Item => {
                if let Some(data) = &frame.data {
                    if let Some(content) = data.get("content").and_then(|v| v.as_str()) {
                        output.push_str(content);
                    } else if let Some(stdout) = data.get("stdout").and_then(|v| v.as_str()) {
                        output.push_str(stdout);
                    } else {
                        output.push_str(&data.to_string());
                    }
                }
            }
            crate::kernel::FrameOp::Error => {
                if let Some(data) = &frame.data {
                    let msg = data.get("message").and_then(|v| v.as_str()).unwrap_or("error");
                    output.push_str(&format!("error: {}", msg));
                }
            }
            crate::kernel::FrameOp::Done => break,
            _ => {}
        }
    }
    output
}
