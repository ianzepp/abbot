mod bundle;
mod config;

pub use bundle::{HandBundleBuilder, HandBundleConfig};
pub use config::HandConfig;

// Hand execution via direct invocation (hand:run syscall).

use std::path::Path;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::hal::llm::{ChatMessage, Role, UnifiedMessage};
use crate::kernel::Frame;
use crate::runtime::Kernel;
use crate::runtime::llm_util::{LlmContentMode, LlmFrameAccumulator};
use crate::syscalls::dispatch::dispatch_tool;

use crate::runtime::SnapshotManager;

// =============================================================================
// SHARED HAND LOOP (used by hand:run syscall)
// =============================================================================

/// Result of executing the hand loop.
pub struct HandResult {
    pub ok: bool,
    pub summary: String,
}

/// Execute the hand's LLM+tool loop as a standalone function.
///
/// This is the core used by the `hand:run` syscall (direct invocation).
/// It builds a hand bundle, runs the LLM+tool loop, and returns a summary.
///
/// Uses the kernel dispatcher's `llm:chat` syscall for LLM calls, so no direct
/// LlmClient dependency is needed.
pub async fn execute_hand_loop(
    prompt: &str,
    context: &str,
    max_iters: usize,
    workspace: &Path,
    actor: &str,
    cancel: CancellationToken,
) -> HandResult {
    let Some(k) = Kernel::get() else {
        return HandResult {
            ok: false,
            summary: "kernel not initialized".to_string(),
        };
    };

    let store = match k.store() {
        Some(s) => s,
        None => {
            return HandResult {
                ok: false,
                summary: "kernel store not attached".to_string(),
            };
        }
    };

    let snapshot = SnapshotManager::new(workspace.to_path_buf(), Some(store.clone())).await;

    let snap = snapshot.get();
    let tools: Vec<crate::hal::llm::ToolSpec> = snap.hand_tools.clone();

    let run_id = Uuid::new_v4().to_string();
    let builder = HandBundleBuilder::new_with_snapshot(store.clone(), snapshot.clone());
    let traits = crate::runtime::AppConfig::global().traits.to_trait_names();
    let bundle_cfg = HandBundleConfig::new(&run_id, "system", prompt, context)
        .with_traits(traits)
        .with_max_iters(max_iters);
    let messages = builder.build(&bundle_cfg).await;

    // Convert UnifiedMessage bundle to ChatMessage for dispatcher-based LLM calls
    let mut chat_messages: Vec<ChatMessage> = unified_to_chat_messages(messages);

    let mut vfs_cwd = String::from("/");
    let mut tool_failure_streak: usize = 0;

    for _iter in 0..max_iters {
        if cancel.is_cancelled() {
            return HandResult {
                ok: false,
                summary: "cancelled".to_string(),
            };
        }

        // Call LLM via kernel dispatcher (same pattern as room runner)
        let llm_result = match call_hand_llm(&chat_messages, &tools, actor, workspace).await {
            Ok(r) => r,
            Err(e) => {
                return HandResult {
                    ok: false,
                    summary: format!("LLM error: {e}"),
                };
            }
        };

        // No tool calls = agent is done
        if llm_result.tool_calls.is_empty() {
            let content = llm_result.content.unwrap_or_default();
            let ok = !content.trim().is_empty();
            return HandResult {
                ok,
                summary: if ok {
                    content.trim().to_string()
                } else {
                    "completed without output".to_string()
                },
            };
        }

        // Execute tool calls (one at a time, strict mode)
        chat_messages.push(ChatMessage::assistant_tool_calls(
            llm_result.tool_calls.clone(),
        ));

        for tc in &llm_result.tool_calls {
            let out = dispatch_tool(
                &tc.function.name,
                &tc.function.arguments,
                actor,
                workspace,
                &vfs_cwd,
            )
            .await;

            // Update VFS CWD if fs:cd succeeded
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

            let success = tool_result_ok(&out);
            if success {
                tool_failure_streak = 0;
            } else {
                tool_failure_streak += 1;
            }

            chat_messages.push(ChatMessage::tool_result(tc.id.clone(), out));
        }

        if tool_failure_streak >= 5 {
            return HandResult {
                ok: false,
                summary: "5 consecutive tool failures".to_string(),
            };
        }
    }

    HandResult {
        ok: false,
        summary: "iteration limit reached without final content".to_string(),
    }
}

/// Convert UnifiedMessage (client::Message) to ChatMessage (openai_compat::ChatMessage).
fn unified_to_chat_messages(messages: Vec<UnifiedMessage>) -> Vec<ChatMessage> {
    messages
        .into_iter()
        .map(|m| match m {
            UnifiedMessage::System(s) => ChatMessage::new(Role::System, s),
            UnifiedMessage::User(s) => ChatMessage::new(Role::User, s),
            UnifiedMessage::Assistant(s) => ChatMessage::new(Role::Assistant, s),
            UnifiedMessage::AssistantToolCalls(calls) => {
                let oai_calls: Vec<crate::hal::llm::ToolCall> = calls
                    .into_iter()
                    .map(|tc| crate::hal::llm::ToolCall {
                        id: tc.id,
                        call_type: "function".to_string(),
                        function: crate::hal::llm::ToolCallFunction {
                            name: tc.name,
                            arguments: tc.arguments.to_string(),
                        },
                    })
                    .collect();
                ChatMessage::assistant_tool_calls(oai_calls)
            }
            UnifiedMessage::ToolResult { id, content, .. } => ChatMessage::tool_result(id, content),
        })
        .collect()
}

/// Call the LLM via llm:chat syscall (dispatcher pattern, same as room runner).
async fn call_hand_llm(
    messages: &[ChatMessage],
    tools: &[crate::hal::llm::ToolSpec],
    actor: &str,
    workspace: &Path,
) -> Result<HandLlmResult, String> {
    let Some(k) = Kernel::get() else {
        return Err("kernel not initialized".to_string());
    };
    let dispatcher = k.dispatcher().await;

    let payload = serde_json::json!({
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
    });

    let req = Frame::req("llm:chat", payload).with_actor(actor.to_string());
    let mut rx = dispatcher.dispatch(req, workspace.to_path_buf(), CancellationToken::new());

    let mut acc = LlmFrameAccumulator::new();

    while let Some(frame) = rx.recv().await {
        match acc.process_frame(&frame) {
            Ok(true) => break,
            Ok(false) => {}
            Err(e) => return Err(e),
        }
    }

    let (content, tool_calls) = acc.into_result(LlmContentMode::EmptyIsNone);

    Ok(HandLlmResult {
        content,
        tool_calls,
    })
}

struct HandLlmResult {
    content: Option<String>,
    tool_calls: Vec<crate::hal::llm::ToolCall>,
}

pub(crate) fn tool_result_ok(tool_result_json: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(tool_result_json) {
        Ok(v) => v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::tool_result_ok;

    #[test]
    fn tool_result_ok_parses_success() {
        assert!(tool_result_ok(r#"{"ok": true}"#));
    }

    #[test]
    fn tool_result_ok_parses_failure() {
        assert!(!tool_result_ok(r#"{"ok": false}"#));
    }

    #[test]
    fn tool_result_ok_rejects_missing_field() {
        assert!(!tool_result_ok(r#"{"status": "ok"}"#));
    }

    #[test]
    fn tool_result_ok_rejects_invalid_json() {
        assert!(!tool_result_ok("not json"));
    }
}
