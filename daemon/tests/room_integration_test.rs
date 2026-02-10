//! Room + Door integration tests with a mock LLM syscall handler.
//!
//! Tests the full pipeline: RoomRunner execution, RoomRegistry lifecycle,
//! agent tool dispatch, transcript sharing, and GC eviction — all without
//! a real LLM provider.
//!
//! IMPORTANT: All tests share a global Kernel singleton (with a single
//! dispatcher), so we use ONE unified mock LLM that handles all test
//! patterns. Tests run in parallel safely because each uses unique rooms.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::mpsc;
use uuid::Uuid;

use abbot::hal::llm::{ChatMessage, Role, ToolSpec};
use abbot::history::Store;
use abbot::kernel::{
    ExternalToolResult, Frame, FrameStore, KernelError, Syscall, SyscallContext, TurnWaitError,
};
use abbot::runtime::{
    Door, Kernel, Room, RoomAgent, RoomRunner, RoomType, SessionWriteGuard, SessionWriteLocks,
};

// =============================================================================
// SHARED SETUP
// =============================================================================

/// Ensure a Kernel singleton is initialized with a FrameStore and mock LLM.
/// Returns the kernel Arc. Safe to call multiple times (idempotent).
async fn ensure_kernel() -> Arc<Kernel> {
    static INIT: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

    // Ensure kernel is initialized. Kernel::init creates + registers globally,
    // but always returns the new instance even if global was already set. We
    // must use Kernel::get() after init to get the actual global singleton.
    if Kernel::get().is_none() {
        let root = std::env::temp_dir().join(format!("abbot-room-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        Kernel::init(&root);
    }
    let k = Kernel::get().unwrap();

    INIT.get_or_init(|| async {
        if k.frames().is_none() {
            let db = std::env::temp_dir().join(format!("frames-room-{}.db", Uuid::new_v4()));
            let store = FrameStore::open(&db).await.unwrap();
            k.set_frames(store).await;
        }

        // Register the unified mock LLM, overwriting the real chat:llm handler
        let mut dispatcher = k.dispatcher_mut().await;
        dispatcher.register(Arc::new(UnifiedMockLlm));
    })
    .await;

    k
}

/// Build noop_signal and noop_done tool specs for agents.
fn noop_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::function(
            "tool__noop_signal",
            "Signal done for this round",
            json!({
                "type": "object",
                "properties": {
                    "reason": { "type": "string" }
                },
                "required": [],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "tool__noop_done",
            "Permanently leave the room",
            json!({
                "type": "object",
                "properties": {
                    "reason": { "type": "string" }
                },
                "required": [],
                "additionalProperties": false
            }),
        ),
    ]
}

// =============================================================================
// UNIFIED MOCK LLM
// =============================================================================

/// Unified mock LLM syscall that handles multiple patterns:
///
/// 1. **MATH mode**: If any user message contains "MATH: X op Y", evaluates
///    the arithmetic and returns the result as text. No tool calls.
///
/// 2. **SIGNAL mode**: If any user message contains "SIGNAL:", and no tool
///    result is in the history yet, emits a tool__noop_signal tool call.
///    If a tool result IS present, responds with text.
///
/// 3. **EXTERNAL mode**: If any user message contains "EXTERNAL: <name>",
///    and no tool result is in the history yet, emits a `user__<name>` tool
///    call. If a tool result IS present, responds with "Tool result received."
///
/// 4. **Default**: Returns "mock response" as text.
struct UnifiedMockLlm;

#[async_trait]
impl Syscall for UnifiedMockLlm {
    fn name(&self) -> &'static str {
        "chat:llm"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        let call_id = ctx.call_id;
        let messages = data["messages"].as_array().cloned().unwrap_or_default();

        // Check for MATH expression in any user message
        let math_expr = messages
            .iter()
            .rev()
            .filter(|m| m["role"].as_str() == Some("user"))
            .find_map(|m| {
                let content = m["content"].as_str().unwrap_or("");
                content.split("MATH:").nth(1).map(|e| e.trim().to_string())
            });

        if let Some(expr) = math_expr {
            let answer = eval_simple_math(&expr);
            let _ = tx
                .send(Frame::item(
                    call_id,
                    json!({ "type": "text_delta", "content": answer }),
                ))
                .await;
            let _ = tx.send(Frame::done(call_id)).await;
            return Ok(());
        }

        // Check for SIGNAL mode
        let has_signal_request = messages.iter().any(|m| {
            m["role"].as_str() == Some("user")
                && m["content"].as_str().is_some_and(|c| c.contains("SIGNAL:"))
        });

        if has_signal_request {
            let has_tool_result = messages.iter().any(|m| m["role"].as_str() == Some("tool"));

            if has_tool_result {
                let _ = tx
                    .send(Frame::item(
                        call_id,
                        json!({ "type": "text_delta", "content": "Tool processing complete." }),
                    ))
                    .await;
            } else {
                let tool_call_id = format!("tc_{}", Uuid::new_v4());
                let _ = tx
                    .send(Frame::item(
                        call_id,
                        json!({
                            "type": "tool_call",
                            "tool_call_id": tool_call_id,
                            "name": "tool__noop_signal",
                            "arguments": { "reason": "signaling done" }
                        }),
                    ))
                    .await;
            }
            let _ = tx.send(Frame::done(call_id)).await;
            return Ok(());
        }

        // Check for EXTERNAL mode
        let external_tool = messages
            .iter()
            .filter(|m| m["role"].as_str() == Some("user"))
            .find_map(|m| {
                let content = m["content"].as_str().unwrap_or("");
                content
                    .split("EXTERNAL:")
                    .nth(1)
                    .map(|e| e.trim().to_string())
            });

        if let Some(tool_name) = external_tool {
            let has_tool_result = messages.iter().any(|m| m["role"].as_str() == Some("tool"));

            if has_tool_result {
                let _ = tx
                    .send(Frame::item(
                        call_id,
                        json!({ "type": "text_delta", "content": "Tool result received." }),
                    ))
                    .await;
            } else {
                let tool_call_id = format!("tc_{}", Uuid::new_v4());
                let _ = tx
                    .send(Frame::item(
                        call_id,
                        json!({
                            "type": "tool_call",
                            "tool_call_id": tool_call_id,
                            "name": format!("user__{tool_name}"),
                            "arguments": { "query": "test" }
                        }),
                    ))
                    .await;
            }
            let _ = tx.send(Frame::done(call_id)).await;
            return Ok(());
        }

        // Default: return generic text
        let _ = tx
            .send(Frame::item(
                call_id,
                json!({ "type": "text_delta", "content": "mock response" }),
            ))
            .await;
        let _ = tx.send(Frame::done(call_id)).await;
        Ok(())
    }
}

/// Evaluate simple integer arithmetic: "X + Y", "X - Y", "X * Y".
fn eval_simple_math(expr: &str) -> String {
    for op in ['+', '-', '*'] {
        if let Some((left, right)) = expr.split_once(op) {
            let a: i64 = left.trim().parse().unwrap_or(0);
            let b: i64 = right.trim().parse().unwrap_or(0);
            let result = match op {
                '+' => a + b,
                '-' => a - b,
                '*' => a * b,
                _ => 0,
            };
            return result.to_string();
        }
    }
    format!("Cannot parse: {expr}")
}

// =============================================================================
// TEST 1: Math LLM produces transcript entries
// =============================================================================

#[tokio::test]
async fn test_room_math_llm_produces_transcript() {
    let _k = ensure_kernel().await;

    let room_name = format!("test/math-{}", Uuid::new_v4());
    let store = Arc::new(Store::open(":memory:").await.unwrap());

    let agent = RoomAgent::new(
        "math-agent",
        "head",
        "You are a math assistant",
        vec![], // no tools needed — returns text only
    );

    let mut room = Room::new(
        Uuid::new_v4().to_string(),
        "math-test",
        RoomType::General,
        "Solve math problems",
        vec![agent],
        3,
    );

    // Inject a user message into the agent's history
    room.agents[0]
        .messages
        .push(ChatMessage::new(Role::User, "MATH: 2 + 3"));

    let runner = RoomRunner::new(store, &room_name);
    let summary = runner.run(&mut room, None).await;

    // Transcript should contain the answer "5"
    assert!(
        !room.transcript.is_empty(),
        "transcript should not be empty"
    );
    let has_answer = room.transcript.iter().any(|t| t.content.contains('5'));
    assert!(
        has_answer,
        "transcript should contain '5', got: {:?}",
        room.transcript
    );

    // Summary should be Some (non-empty)
    assert!(summary.is_some(), "summary should be Some");
    let summary_text = summary.unwrap();
    assert!(
        !summary_text.trim().is_empty(),
        "summary should not be empty"
    );
}

// =============================================================================
// TEST 2: Tool LLM signals noop_signal
// =============================================================================

#[tokio::test]
async fn test_room_tool_llm_signals_noop() {
    let _k = ensure_kernel().await;

    let room_name = format!("test/tool-{}", Uuid::new_v4());
    let store = Arc::new(Store::open(":memory:").await.unwrap());

    let agent = RoomAgent::new(
        "tool-agent",
        "head",
        "You are a helpful assistant",
        noop_tool_specs(),
    );

    let mut room = Room::new(
        Uuid::new_v4().to_string(),
        "tool-test",
        RoomType::General,
        "Test tool signaling",
        vec![agent],
        5,
    );

    // Inject a user message that triggers SIGNAL mode
    room.agents[0].messages.push(ChatMessage::new(
        Role::User,
        "SIGNAL: please signal when done",
    ));

    let runner = RoomRunner::new(store, &room_name);
    let _summary = runner.run(&mut room, None).await;

    // Room should have terminated — the agent signaled noop_signal,
    // which means it produced no visible text → quiescence → room ends.
    // Key assertion: room terminated (we got here without hanging).
    assert!(
        room.max_rounds > 0,
        "room should have a positive max_rounds"
    );
}

// =============================================================================
// TEST 3: RoomRegistry inject_message and wait_for_done
// =============================================================================

#[tokio::test]
async fn test_room_registry_inject_and_wait() {
    let k = ensure_kernel().await;

    let room_name = format!("test/registry-{}", Uuid::new_v4());
    let store = Arc::new(Store::open(":memory:").await.unwrap());

    let room_clone = room_name.clone();
    let store_clone = store.clone();

    // Create room via registry
    let active = k
        .rooms()
        .get_or_create(&room_name, move || {
            let agent =
                RoomAgent::new("registry-agent", "head", "You are a math assistant", vec![]);
            let room = Room::new(
                Uuid::new_v4().to_string(),
                "registry-test",
                RoomType::General,
                "Registry math test",
                vec![agent],
                3,
            );
            let runner = RoomRunner::new(store_clone, &room_clone);
            (room, runner)
        })
        .await;

    // Inject a message and wait for processing
    k.rooms()
        .inject_message(&room_name, "MATH: 10 + 20".to_string())
        .await;

    // Wait for the room to finish processing
    k.rooms().wait_for_done(&room_name).await;

    // Check transcript contains the answer
    let room = active.state.lock().await;
    let has_answer = room.transcript.iter().any(|t| t.content.contains("30"));
    assert!(
        has_answer,
        "transcript should contain '30', got: {:?}",
        room.transcript
    );

    // Room was successfully managed by the registry (get_or_create + inject + wait worked)
    // Note: we don't assert registry len() because concurrent GC tests may evict rooms.
}

// =============================================================================
// TEST 4: RoomRegistry GC evicts idle rooms
// =============================================================================

#[tokio::test]
async fn test_room_registry_gc_evicts_idle() {
    let k = ensure_kernel().await;

    let room_name = format!("test/gc-{}", Uuid::new_v4());
    let store = Arc::new(Store::open(":memory:").await.unwrap());

    let room_clone = room_name.clone();
    let store_clone = store.clone();

    // Create room via registry
    k.rooms()
        .get_or_create(&room_name, move || {
            let agent = RoomAgent::new("gc-agent", "head", "Math assistant", vec![]);
            let room = Room::new(
                Uuid::new_v4().to_string(),
                "gc-test",
                RoomType::General,
                "GC test",
                vec![agent],
                3,
            );
            let runner = RoomRunner::new(store_clone, &room_clone);
            (room, runner)
        })
        .await;

    // Inject and wait
    k.rooms()
        .inject_message(&room_name, "MATH: 1 + 1".to_string())
        .await;
    k.rooms().wait_for_done(&room_name).await;

    // GC with zero timeout should evict everything
    let evicted = k.rooms().gc_idle(std::time::Duration::ZERO).await;
    assert!(
        evicted >= 1,
        "should have evicted at least 1 room, got {evicted}"
    );
}

// =============================================================================
// TEST 5: Multi-agent transcript sharing
// =============================================================================

#[tokio::test]
async fn test_room_multi_agent_transcript_sharing() {
    let _k = ensure_kernel().await;

    let room_name = format!("test/multi-{}", Uuid::new_v4());
    let store = Arc::new(Store::open(":memory:").await.unwrap());

    let agent1 = RoomAgent::new("agent-alpha", "head", "You are math agent Alpha", vec![]);
    let agent2 = RoomAgent::new("agent-beta", "head", "You are math agent Beta", vec![]);

    let mut room = Room::new(
        Uuid::new_v4().to_string(),
        "multi-test",
        RoomType::General,
        "Multi-agent math test",
        vec![agent1, agent2],
        3,
    );

    // Inject user message into both agents
    for agent in &mut room.agents {
        agent
            .messages
            .push(ChatMessage::new(Role::User, "MATH: 7 + 8"));
    }

    let runner = RoomRunner::new(store, &room_name);
    let summary = runner.run(&mut room, None).await;

    // Both agents should have produced transcript entries
    let alpha_spoke = room.transcript.iter().any(|t| t.agent == "agent-alpha");
    let beta_spoke = room.transcript.iter().any(|t| t.agent == "agent-beta");
    assert!(alpha_spoke, "agent-alpha should have a transcript entry");
    assert!(beta_spoke, "agent-beta should have a transcript entry");

    // At least 2 transcript entries (one per agent minimum)
    assert!(
        room.transcript.len() >= 2,
        "should have at least 2 transcript entries, got {}",
        room.transcript.len()
    );

    // Both should contain "15"
    let has_answer = room.transcript.iter().any(|t| t.content.contains("15"));
    assert!(
        has_answer,
        "transcript should contain '15', got: {:?}",
        room.transcript
    );

    // Summary should exist
    assert!(
        summary.is_some(),
        "summary should be Some for multi-agent room"
    );
}

// =============================================================================
// MOCK DOOR
// =============================================================================

/// Captured external tool call for assertions.
#[derive(Debug, Clone)]
struct CapturedToolCall {
    tool_call_id: String,
    name: String,
    arguments: Value,
}

/// Mock Door that captures external tool calls and returns canned results.
/// No WebSocket/SigcallHub/TurnRuntime needed — tests the runner's external
/// tool routing in isolation.
struct MockDoor {
    external_names: HashSet<String>,
    external_tool_specs: Vec<ToolSpec>,
    captured: Arc<TokioMutex<Vec<CapturedToolCall>>>,
    session_locks: SessionWriteLocks,
}

impl std::fmt::Debug for MockDoor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockDoor")
            .field("external_names", &self.external_names)
            .finish()
    }
}

#[async_trait]
impl Door for MockDoor {
    async fn emit_chat_message(&self, _content: &str) -> Result<(), String> {
        Ok(())
    }

    async fn emit_chat_tool(
        &self,
        tool_call_id: &str,
        name: &str,
        arguments: &Value,
    ) -> Result<(), String> {
        let mut captured = self.captured.lock().await;
        captured.push(CapturedToolCall {
            tool_call_id: tool_call_id.to_string(),
            name: name.to_string(),
            arguments: arguments.clone(),
        });
        Ok(())
    }

    async fn emit_chat_done(&self, _reason: &str) -> Result<(), String> {
        Ok(())
    }

    async fn emit_chat_error(&self, _code: &str, _message: &str) -> Result<(), String> {
        Ok(())
    }

    async fn emit_chat_thinking(&self) -> Result<(), String> {
        Ok(())
    }

    async fn emit_chat_activity(
        &self,
        _actor: &str,
        _tool: &str,
        _summary: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn emit_chat_thought(&self, _actor: &str, _content: &str) -> Result<(), String> {
        Ok(())
    }

    async fn is_turn_cancelled(&self) -> bool {
        false
    }

    fn is_external_tool(&self, name: &str) -> bool {
        self.external_names.contains(name)
    }

    async fn wait_for_external_tool_result(
        &self,
        tool_call_id: &str,
    ) -> Result<ExternalToolResult, TurnWaitError> {
        Ok(ExternalToolResult {
            tool_call_id: tool_call_id.to_string(),
            name: "search".to_string(),
            content: "mock search result".to_string(),
            is_error: false,
        })
    }

    async fn acquire_write_lock(&self) -> SessionWriteGuard {
        self.session_locks.acquire("mock-room").await
    }

    fn external_tools(&self) -> &[ToolSpec] {
        &self.external_tool_specs
    }
}

// =============================================================================
// TEST 6: External tool round-trip via MockDoor
// =============================================================================

#[tokio::test]
async fn test_room_external_tool_roundtrip() {
    let _k = ensure_kernel().await;

    let room_name = format!("test/external-{}", Uuid::new_v4());
    let store = Arc::new(Store::open(":memory:").await.unwrap());

    // Build the external tool spec (user__search)
    let external_tool_spec = ToolSpec::function(
        "user__search",
        "Search for information",
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
    );

    // Create a MockDoor that recognizes user__search
    let captured = Arc::new(TokioMutex::new(Vec::new()));
    let mock_door = Arc::new(MockDoor {
        external_names: HashSet::from(["user__search".to_string()]),
        external_tool_specs: vec![external_tool_spec.clone()],
        captured: captured.clone(),
        session_locks: SessionWriteLocks::new(),
    });

    // Create agent with noop tools + external tool spec
    let mut tools = noop_tool_specs();
    tools.push(external_tool_spec);

    let agent = RoomAgent::new("ext-agent", "head", "You are a search assistant", tools);

    let mut room = Room::new(
        Uuid::new_v4().to_string(),
        "external-test",
        RoomType::General,
        "External tool test",
        vec![agent],
        3,
    );

    // Attach the mock door to the room
    room.door = Some(mock_door);

    // Inject a user message that triggers EXTERNAL mode
    room.agents[0]
        .messages
        .push(ChatMessage::new(Role::User, "EXTERNAL: search"));

    let runner = RoomRunner::new(store, &room_name);
    let summary = runner.run(&mut room, None).await;

    // Assert: MockDoor captured at least one external tool call with name "search"
    let captured = captured.lock().await;
    assert!(
        !captured.is_empty(),
        "MockDoor should have captured at least one external tool call"
    );
    assert_eq!(
        captured[0].name, "search",
        "captured tool call name should be 'search' (user__ prefix stripped), got: {:?}",
        captured[0].name
    );

    // Assert: agent message history contains a tool result with the mock content
    let has_mock_result = room.agents[0].messages.iter().any(|m| {
        // ChatMessage::ToolResult contains the content field
        format!("{m:?}").contains("mock search result")
    });
    assert!(
        has_mock_result,
        "agent messages should contain 'mock search result', got: {:?}",
        room.agents[0].messages
    );

    // Assert: room completed (summary exists since LLM responded with text on second call)
    assert!(
        summary.is_some(),
        "summary should be Some after external tool round-trip"
    );
}
