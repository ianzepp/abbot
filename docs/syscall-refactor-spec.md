# Syscall Refactor Specification (v2)

## Overview

This document specifies a clean rewrite of Abbot's frame/syscall chat processing to establish consistency, clarity, and to remove legacy compatibility layers.

Problems addressed:

- Inconsistent semantics (`Frame.name` vs `data.kind`)
- Authorship buried in payloads (actor should be a frame field)
- Internal reasoning leaking into user-visible chat
- External tool flow limited to a single tool call per response (redirect closes connection)
- Ambiguous lifecycle: what ends a turn vs what ends work

Non-goals:

- Preserving backwards compatibility in frame flow or syscall response shapes

## Design Principles

1. `name` is always `<namespace>:<verb>`
2. `actor` indicates authorship (a frame field)
3. Thinking is explicit and never forwarded as chat
4. A turn has a clean lifecycle (`chat:done` / `chat:error`), never implicit redirect
5. Multiple external tools are emitted as a batch before `chat:done`

---

## Core Concepts

### Frame

- A `Frame` is the unit on the wire.
- A syscall is requested with `FrameOp::Req`.
- A syscall responds on its *syscall response stream* with `Ok/Item/Bytes/Done/Error`.

### Turn

- A turn is the user-visible exchange keyed by `(scope, reply_to)`.
- A turn may span multiple HTTP requests (segments) if external tools are required.
- The turn stream is what the client consumes (text deltas, tool calls, done/error).

### Need

- A need is an internal schedulable unit of head work (priority, fairness, pooling).
- A single need may span multiple turn segments.
- External tool results resume the *same need* (continuation), not a new need.

### Segment

- A segment is one continuous client connection streaming turn output.
- When external tools are required, the head ends the current segment by emitting tool calls followed by `chat:done`.
- The need remains active/paused; the next segment resumes after tool results arrive.

---

## New Syscall Taxonomy

### Chat Namespace (`chat:*`)

| Syscall | Direction | Purpose |
|---------|-----------|---------|
| `chat:message` | Ingress/Egress | A user-visible chat message exists (user -> system, head -> user) |
| `chat:tool` | Egress | External tool call for client to execute |
| `chat:tool_result` | Ingress | External tool result for a prior tool call (resumes the same need) |
| `chat:done` | Egress | Segment complete; the connection can close |
| `chat:error` | Egress | Segment failed; the connection should close |
| `chat:cancel` | Ingress | Client disconnected; cancel this turn if possible |

### LLM Namespace (`llm:*`)

| Syscall | Direction | Purpose |
|---------|-----------|---------|
| `llm:chat` | Internal | Send messages to LLM provider, receive response |

### Need Namespace (`need:*`) - Unchanged

| Syscall | Purpose |
|---------|---------|
| `need:enqueue` | Queue work for a head |
| `need:lease` | Head claims work |
| `need:fulfill` | Head completed work |

### Task Namespace (`task:*`) - Unchanged

| Syscall | Purpose |
|---------|---------|
| `task:enqueue` | Queue work for a hand |
| `task:lease` | Hand claims work |
| `task:complete` | Hand completed work |

---

## Frame Structure

```rust
pub struct Frame {
    pub id: Uuid,
    pub op: FrameOp,              // req, ok, done, error, item, bytes, event
    pub name: Option<String>,     // Always <namespace>:<verb> for Req; optional on turn stream frames for filtering/diagnostics
    pub parent_id: Option<Uuid>,  // Correlation to parent request
    pub actor: Option<String>,    // Who sent: "user", "head/<id>", "hand/<id>", "system"
    pub data: Option<Value>,      // Operation-specific payload
}
```

### Streams

There are two streams to keep distinct:

- Syscall response stream: frames sent from a syscall implementation back to the syscall caller.
- Turn stream: frames sent to the client for `(scope, reply_to)` (the user-visible stream).

In this refactor, `chat:*` syscalls are *side-effecting emitters*:

- The `chat:*` request (`op=req`) carries the semantic payload (content, tool call, etc.).
- The syscall implementation emits the corresponding frame(s) onto the turn stream.
- The syscall response stream for `chat:*` is typically a simple `ok` acknowledging it was emitted.

Note on `Frame.name` outside `op=req`:

- `Frame.name` is required for syscall requests.
- For turn stream frames, `Frame.name` MAY be set to the corresponding chat syscall name (e.g. `chat:message`, `chat:tool`) to support monitoring/filters.
- Clients MUST NOT rely on `Frame.name` to interpret turn stream payloads; they MUST use `op` and `data.type`.

Turn stream addressing and lifecycle:

- The turn stream is addressed by `(scope, reply_to)`.
- Ingress is responsible for ensuring a turn stream exists before it enqueues work (so head output is never dropped).
- A segment is created when a client connects and begins streaming the turn; it ends when the head emits `chat:done` or `chat:error`.

Tool-result-only resumptions:

- `chat:tool_result` may be submitted when there is no active segment (no client currently streaming).
- In that case, `chat:tool_result` MUST update turn state and wake the head, but MUST NOT assume a client is connected.

Important: `FrameOp::Done` and `chat:done` are different concepts.

- `FrameOp::Done` is a frame op used on a syscall response stream.
- `chat:done` is a chat syscall name whose effect is to end a turn segment on the turn stream.

Note on `FrameOp::Bytes`:

- `FrameOp::Bytes` is intended to be a raw bytes channel.
- This refactor does not use `FrameOp::Bytes` for chat text. Chat text is emitted as `op=item` frames.
- A follow-up change should enforce/restore `Bytes` as truly raw bytes end-to-end.

### Remove `FrameOp::Redirect`

The `Redirect` op is removed entirely. External tool calls use `chat:tool` syscall instead.

---

## Data Payloads

### `chat:message`

```json
{
  "scope": "main" | "session/<hash>",
  "content": "Hello, how can I help?",
  "reply_to": "<uuid>"            // Required: turn correlation id
}
```

Actor field on frame indicates sender:
- `actor: "user"` - user sent this message
- `actor: "head/<id>"` - head sent this message

Behavior rules (normative):

- If `actor == "user"`, `chat:message` MUST log the message and MUST enqueue (or attach to) the need for `(scope, reply_to)`.
- If `actor` starts with `head/`, `chat:message` MUST log the message and MUST emit user-visible text onto the turn stream for `(scope, reply_to)`.
- `chat:message` MUST NOT enqueue work for messages authored by heads.
- If `actor == "user"`, `chat:message` MUST NOT emit user-visible output.

Notes:

- Thinking is not represented as `chat:message`. Thinking is emitted by `llm:chat` as `item {type:"thinking"}` and is logged, not forwarded.
- User-visible text is emitted on the turn stream as `op=item` with `data.type == "text_delta"`.

### `chat:tool`

```json
{
  "scope": "main" | "session/<hash>",
  "reply_to": "<uuid>",
  "tool_call_id": "<id>",
  "name": "user__read_file",
  "arguments": { "path": "foo.txt" }
}
```

Field types:

- `arguments` MUST be a JSON object. (If a provider returns stringified JSON arguments, that normalization happens in `llm:chat`.)

Rendezvous semantics (required for resuming the same need):

- When the head emits a `chat:tool`, it MUST register the tool call as pending in a kernel-owned
  turn state keyed by `(scope, reply_to)` and `tool_call_id`.
- The head then blocks the active need until the pending tool call(s) are satisfied.
- The head MUST NOT `need:fulfill` while external tool calls are pending.
- Tool results are correlated strictly by `tool_call_id` (scoped by `(scope, reply_to)`).

Turn stream encoding:

- `chat:tool` emits a turn stream frame with `op=item` and `data.type == "tool_call"`.
- The emitted item payload uses `tool_call_id` (not `id`).
- Clients MUST interpret tool calls by `op=item` + `data.type == "tool_call"`.
- Clients MUST NOT interpret tool calls by checking `Frame.name`.

### `chat:tool_result`

```json
{
  "scope": "main" | "session/<hash>",
  "reply_to": "<uuid>",
  "tool_call_id": "<id>",
  "name": "user__read_file",
  "content": "... tool output ...",
  "is_error": false
}
```

Field types:

- `content` MUST be a string. If the tool output is structured JSON, it MUST be encoded as a JSON string.

Semantics:

- Resumes the prior in-flight need for `(scope, reply_to)`.
- Must not enqueue a new need.
- The head correlates results by `tool_call_id`.

Delivery semantics:

- `chat:tool_result` MUST look up a pending external tool call registration for
  `(scope, reply_to, tool_call_id)`.
- If found, it MUST deliver the result and wake the waiting head (unblocking the active need).
- If not found, it MUST still be logged, and SHOULD return an error to the caller
  (e.g. unknown `tool_call_id`) rather than silently dropping it.

Cancellation interaction:

- If the turn is cancelled, `chat:tool_result` MUST still be logged.
- If the turn is cancelled, `chat:tool_result` SHOULD return an error to the caller (e.g. cancelled).
- If the head is blocked waiting on the result, the runtime SHOULD still wake it so it can observe cancellation and exit.

### `chat:done`

```json
{
  "scope": "main" | "session/<hash>",
  "reply_to": "<uuid>",
  "reason": "complete" | "awaiting_tools"
}
```

Semantics:

- Ends the current segment (client connection may close).
- Does not imply the need is fulfilled; the need may remain active/paused (e.g., awaiting external tools).

Client guidance:

- If `reason == "awaiting_tools"`, the client SHOULD expect that the turn will resume only after it submits one or more `chat:tool_result` frames.
- If `reason == "complete"`, the client SHOULD treat the turn as finished.

Turn stream encoding:

- `chat:done` MUST emit a final `op=item` frame with `data.type == "done"` and include the `reason` field.
- After the done item, `chat:done` MUST emit a terminal `op=done` frame and close the segment.

### `chat:error`

```json
{
  "scope": "main" | "session/<hash>",
  "reply_to": "<uuid>",
  "code": "E_*",
  "message": "Human-readable error"
}
```

Turn stream encoding:

- `chat:error` MUST be the last emission in a segment.
- `chat:error` MUST emit `op=error` with `data` containing `{code, message}` and close the segment.

### `chat:cancel`

```json
{
  "scope": "main" | "session/<hash>",
  "reply_to": "<uuid>",
  "reason": "client_disconnect"
}
```

Semantics:

- Requests cancellation of the turn keyed by `(scope, reply_to)`.
- Cancellation is best-effort: it should prevent further tool dispatch and future LLM calls; in-flight work may still complete.
- This refactor defines cancellation as a kernel-owned turn-cancellation registry keyed by `(scope, reply_to)`.

Mandatory head check points (normative):

- Before starting any `llm:chat` call.
- Before emitting any `chat:tool` batch.
- Before blocking on internal tool tasks.
- Before `need:fulfill`.

### `llm:chat` (response items are a hard contract)

Request:
```json
{
  "model": "anthropic/claude-3-5-haiku-latest",
  "messages": [...],
  "tools": [...]
}
```

Response uses `op=item` frames for each logical piece:
```json
// Text content
{ "type": "text_delta", "content": "Hello!" }

// Thinking (extracted from <thinking> tags)
{ "type": "thinking", "content": "I should greet the user..." }

// Tool call
{ "type": "tool_call", "tool_call_id": "...", "name": "...", "arguments": {...} }
```

Followed by `op=done` when complete.

---

## Request Flow

### User Sends Message

```
1. HTTP POST /v1/chat/completions
   - Ingress allocates `reply_to` (a new UUID) for this turn if one is not already established by the protocol.
   ↓
2. Ingress creates chat:message syscall
   Frame {
     op: Req,
     name: "chat:message",
     actor: "user",
     data: { scope: "main", reply_to: "<uuid>", content: "Hello" }
    }
   ↓
3. ChatHandler receives chat:message
   - Stores message to conversation log
   - Creates or resumes a turn keyed by (scope, reply_to)
   - Enqueues need:enqueue referencing (scope, reply_to)
   ↓
4. HeadService leases via need:lease
   ↓
5. Head builds context, creates llm:chat syscall
   ↓
6. LlmClient sends to OpenAI-compat provider
   ↓
7. Response arrives as item frames:
   ← item { type: "thinking", content: "..." }
   ← item { type: "text", content: "Hi there!" }
   ← done
   ↓
8. Head processes items:
   - thinking → logged, NOT sent to chat
   - text → emit chat:message with actor="head/<id>"
   ↓
9. Head emits chat:done
   ↓
10. IngressHub sees chat:done, closes HTTP response
```

### With Internal Tool Calls

```
7. Response arrives:
    ← item { type: "text", content: "Let me check..." }
    ← item { type: "tool_call", tool_call_id: "...", name: "head__fs_read", ... }
    ← done
   ↓
8. Head processes:
   - text → chat:message (streamed to client)
   - tool_call (internal) → task:enqueue, wait for result
   ↓
9. Hand executes tool, returns result
   ↓
10. Head sends another llm:chat with tool result
   ↓
11. Loop until no more internal tool calls
   ↓
12. Head emits chat:done
```

### With External Tool Calls

```
7. Response arrives:
   ← item { type: "text", content: "I'll read that file..." }
   ← item { type: "tool_call", name: "user__read_file", ... }  // external
   ← item { type: "tool_call", name: "user__grep", ... }       // external
   ← done
   ↓
8. Head processes:
   - text → chat:message
   - Collects external tool calls (does NOT emit yet)
   ↓
9. After all internal processing complete:
    - chat:tool for each external tool call
    - chat:done (reason="awaiting_tools")
   ↓
10. IngressHub streams tool calls to client, closes connection
   ↓
11. Client executes tools
   ↓
12. Client submits tool results (no new user message): chat:tool_result
   ↓
13. Head resumes the same need (continuation), feeds tool results to LLM, continues
```

Ordering rule (normative):

- Within a segment, the head MAY emit `chat:message` text at any time before it begins emitting `chat:tool`.
- Once the head emits the first `chat:tool` in a segment, it MUST NOT emit any further `chat:message` in that same segment.
- A segment that emits any `chat:tool` MUST end with `chat:done` where `reason == "awaiting_tools"`.

### Client Disconnects Mid-Processing

```
1. Client closes HTTP connection
   ↓
2. IngressHub detects disconnect
   ↓
3. IngressHub emits chat:cancel
   Frame {
     op: Req,
     name: "chat:cancel",
     actor: "system",
     data: { scope: "main", reply_to: "<uuid>", reason: "client_disconnect" }
   }
   ↓
4. HeadService receives, cancels in-flight need
   - No further chat/tool frames are emitted
   - Internal tool tasks can complete or be cancelled
   - No response sent (connection gone)
```

---

## Code Changes Required

## Internal Structure (Clean Rewrite)

This refactor is intentionally structured so that internal correctness and simplicity come first.
All external protocols (OpenAI-compatible HTTP, Anthropic-compatible HTTP, web chat SSE) become
thin adapters over the same internal turn pipeline.

### Canonical Turn Runtime (Kernel-Owned)

Introduce a single kernel-owned module (e.g. `src/kernel/turns.rs`) that is the canonical owner
of turn state keyed by `(scope, reply_to)`.

Responsibilities:

- Turn stream lifecycle and addressing (`(scope, reply_to)`)
- Cancellation state for a turn
- External tool rendezvous state (pending tool calls + result delivery)
- Optional: tracking the active need id for a turn (debugging/observability only)

Rules:

- Only `chat:*` syscalls are allowed to mutate or emit turn state.
- `HeadService` MUST NOT call `SigcallHub::send/close` directly; it must dispatch `chat:*` syscalls.

Suggested in-kernel API surface (illustrative):

```rust
// src/kernel/turns.rs (illustrative)
pub struct TurnKey { pub scope: String, pub reply_to: Uuid }

pub struct TurnRuntime {
    // state keyed by TurnKey
}

impl TurnRuntime {
    pub fn ensure_stream(&self, key: &TurnKey);
    pub fn emit_text(&self, key: &TurnKey, text: &str);
    pub fn emit_item(&self, key: &TurnKey, item: serde_json::Value);
    pub fn close_segment(&self, key: &TurnKey);

    pub fn cancel(&self, key: &TurnKey, reason: &str);
    pub fn is_cancelled(&self, key: &TurnKey) -> bool;

    pub fn register_external_tool(&self, key: &TurnKey, tool_call_id: &str);
    pub fn deliver_external_tool_result(&self, key: &TurnKey, tool_call_id: &str, content: &str) -> Result<(), KernelError>;
}
```

This is a refactor target, not a strict implementation; the goal is a single well-defined owner.

### Consider Splitting `chat:message` by Direction (Optional)

`chat:message` has dual semantics (user ingress vs head egress) keyed by `actor`.
For maximum internal clarity, the implementation MAY be split into two syscalls while preserving
the public taxonomy:

- `chat:ingress` (actor must be `user`/`system`): log + enqueue/resume need
- `chat:emit` (actor must start with `head/`): log + emit to turn stream

If you keep a single `chat:message`, it MUST retain the normative behavior rules defined above
(never enqueue on head-authored messages, never emit on user-authored messages).

Adapter guidance:

- External protocol adapters (OpenAI/Anthropic/web chat) MAY call `chat:ingress` / `chat:emit` directly if implemented.
- If `chat:ingress` / `chat:emit` are not implemented, adapters MUST call `chat:message` and rely on actor-based rules.

### 1. Frame Definition (`src/kernel/frame.rs`)

**Remove:**
- `FrameOp::Redirect` variant

**No other changes** - Frame struct is already correct.

---

### 2. New Syscall: `chat:message` (`src/syscalls/chat.rs` - NEW FILE)

```rust
pub struct ChatMessage;

impl Syscall for ChatMessage {
    fn name(&self) -> &'static str { "chat:message" }

    async fn execute(&self, ctx: SyscallContext, data: Value, tx: FrameSender) -> Result<(), KernelError> {
        let scope = data["scope"].as_str().ok_or(...)?;
        let content = data["content"].as_str().ok_or(...)?;
        let reply_to = Uuid::parse_str(data["reply_to"].as_str().ok_or(...)?)?;
        let actor = ctx.actor_str(); // "user" or "head/<id>" (illustrative)

        // 1) Append to conversation log
        // 2) If actor == user: enqueue need for (scope, reply_to)
        // 3) If actor starts_with head/: emit user-visible text to the turn stream for (scope, reply_to)

        tx.send(Frame::ok(ctx.frame_id(), json!({"sent": true}))).await?;
        Ok(())
    }
}
```

---

### 3. New Syscall: `chat:tool` (`src/syscalls/chat.rs`)

```rust
pub struct ChatTool;

impl Syscall for ChatTool {
    fn name(&self) -> &'static str { "chat:tool" }

    async fn execute(&self, ctx: SyscallContext, data: Value, tx: FrameSender) -> Result<(), KernelError> {
        let scope = data["scope"].as_str().ok_or(...)?;
        let reply_to = Uuid::parse_str(data["reply_to"].as_str().ok_or(...)?)?;
        let tool_call_id = data["tool_call_id"].as_str().ok_or(...)?;
        let name = data["name"].as_str().ok_or(...)?;
        let arguments = &data["arguments"];

        // Send to the turn stream (client will see this as a tool call)
        let k = Kernel::get().ok_or(...)?;
        k.sigcalls().send(scope, reply_to, Frame::item(
            ctx.frame_id(),
            json!({
                "type": "tool_call",
                "tool_call_id": tool_call_id,
                "name": name,
                "arguments": arguments,
            })
        ).with_name("chat:tool"));

        tx.send(Frame::ok(ctx.frame_id(), json!({"sent": true}))).await?;
        Ok(())
    }
}
```

---

### 4. New Syscall: `chat:tool_result` (`src/syscalls/chat.rs`)

- Accepts external tool output for a prior `chat:tool` call.
- Logs the tool result.
- Delivers the result to the head waiting on `(scope, reply_to, tool_call_id)`.
- Must not enqueue a new need.

---

### 5. New Syscall: `chat:done` (`src/syscalls/chat.rs`)

```rust
pub struct ChatDone;

impl Syscall for ChatDone {
    fn name(&self) -> &'static str { "chat:done" }

    async fn execute(&self, ctx: SyscallContext, data: Value, tx: FrameSender) -> Result<(), KernelError> {
        let scope = data["scope"].as_str().ok_or(...)?;
        let reply_to = Uuid::parse_str(data["reply_to"].as_str().ok_or(...)?)?;

        let reason = data["reason"].as_str().unwrap_or("complete");

        // Emit a done item with reason, then send done frame and close stream
        let k = Kernel::get().ok_or(...)?;
        k.sigcalls().send(
            scope,
            reply_to,
            Frame::item(ctx.frame_id(), json!({"type": "done", "reason": reason})).with_name("chat:done"),
        );
        k.sigcalls().send(scope, reply_to, Frame::done(ctx.frame_id()));
        k.sigcalls().close(scope, reply_to);

        tx.send(Frame::ok(ctx.frame_id(), json!({"closed": true}))).await?;
        Ok(())
    }
}
```

---

### 6. New Syscall: `chat:error` (`src/syscalls/chat.rs`)

- Emits a terminal error on the turn stream for `(scope, reply_to)` and closes the segment.
- The syscall response stream should still return a structured `ok`/`error` for the caller.

---

### 7. New Syscall: `chat:cancel` (`src/syscalls/chat.rs`)

```rust
pub struct ChatCancel;

impl Syscall for ChatCancel {
    fn name(&self) -> &'static str { "chat:cancel" }

    async fn execute(&self, ctx: SyscallContext, data: Value, tx: FrameSender) -> Result<(), KernelError> {
        let scope = data["scope"].as_str().ok_or(...)?;
        let reply_to = Uuid::parse_str(data["reply_to"].as_str().ok_or(...)?)?;

        // Mark (scope, reply_to) cancelled in the kernel turn-cancellation registry.
        // Heads consult this registry to abort before/after expensive steps.

        tx.send(Frame::ok(ctx.frame_id(), json!({"cancelled": true}))).await?;
        Ok(())
    }
}
```

---

### 8. Register Chat Syscalls (`src/syscalls/mod.rs`)

```rust
mod chat;  // NEW

pub fn register_all(dispatcher: &mut KernelDispatcher) {
    // ... existing registrations ...
    chat::register(dispatcher);  // NEW
}
```

```rust
// src/syscalls/chat.rs
pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(ChatMessage));
    dispatcher.register(Arc::new(ChatTool));
    dispatcher.register(Arc::new(ChatToolResult));
    dispatcher.register(Arc::new(ChatDone));
    dispatcher.register(Arc::new(ChatError));
    dispatcher.register(Arc::new(ChatCancel));
}
```

---

### 9. IngressHub Changes (`src/server/ingress_hub.rs`)

**Current:** Creates `log:append` with `kind: "chat:user"`, then `need:enqueue`

**New:** Creates `chat:message` syscall which handles logging and need creation.

Tool results are submitted via `chat:tool_result` (not `tool:result`, not `chat:message`).

```rust
// BEFORE (lines 28-45):
let log_frame = Frame::req("log:append", json!({
    "kind": "chat:user",
    "scope": scope,
    "data": { "content": content },
}));

// AFTER:
let chat_frame = Frame::req("chat:message", json!({
    "scope": scope,
    "reply_to": reply_to,
    "content": content,
})).with_actor("user");
```

**File:** `src/server/ingress_hub.rs`
- `submit_user_turn()` (line ~28-45)
- `submit_tool_results()` (line ~47-124) - similar changes for tool results

---

### 10. HeadService Changes (`src/runtime/head_service.rs`)

**Major changes needed:**

#### a. Replace redirect with chat:tool + chat:done (batch external tools)

**Current (lines 705-721):**
```rust
let redirect = Frame::redirect(reply_to, json!({
    "tool_call_id": tc.id,
    "name": client_name,
    "arguments": tc.function.arguments,
}));
k.sigcalls().send(&scope, reply_to, redirect);
k.sigcalls().close(&scope, reply_to);
```

**New:**
```rust
// Collect all external tool calls first
let external_calls: Vec<_> = tool_calls.iter()
    .filter(|tc| tc.function.name.starts_with("user__"))
    .collect();

// After all internal processing complete, emit external calls (batch)
for tc in external_calls {
    dispatcher.dispatch(Frame::req("chat:tool", json!({
        "scope": scope,
        "reply_to": reply_to,
        "tool_call_id": tc.id,
        "name": tc.function.name,
        "arguments": tc.function.arguments,
    })).with_actor(format!("head/{}", self.head_id))).await?;
}

// Then close the segment
dispatcher.dispatch(Frame::req("chat:done", json!({
    "scope": scope,
    "reply_to": reply_to,
})).with_actor(format!("head/{}", self.head_id))).await?;
```

#### b. Replace chat:message emission

**Current (lines 1093-1105):**
```rust
let frame = Frame::bytes(reply_to, json!({"text": content}))
    .with_name("chat:message");
k.sigcalls().send(&scope, reply_to, frame);
```

**New:**
```rust
dispatcher.dispatch(Frame::req("chat:message", json!({
    "scope": scope,
    "content": content,
    "reply_to": reply_to,
})).with_actor(format!("head/{}", self.head_id))).await?;
```

#### c. Parse thinking tags from LLM response

Thinking parsing location:

- `llm:chat` is the canonical place to extract `<thinking>...</thinking>` and emit it as `item {type:"thinking"}`.
- `HeadService` SHOULD NOT parse thinking tags itself.
- The only acceptable reason for `HeadService` to parse thinking tags is as a temporary migration aid while `llm:chat` is being refactored.

**Temporary migration logic (delete after `llm:chat` itemization is in place):**
```rust
fn parse_llm_content(content: &str) -> (Option<String>, Option<String>) {
    // Extract <thinking>...</thinking> blocks
    let thinking_re = Regex::new(r"<thinking>([\s\S]*?)</thinking>").unwrap();

    let mut thinking_parts = Vec::new();
    let mut visible_content = content.to_string();

    for cap in thinking_re.captures_iter(content) {
        thinking_parts.push(cap[1].to_string());
        visible_content = visible_content.replace(&cap[0], "");
    }

    let thinking = if thinking_parts.is_empty() {
        None
    } else {
        Some(thinking_parts.join("\n"))
    };

    let visible = visible_content.trim();
    let visible = if visible.is_empty() { None } else { Some(visible.to_string()) };

    (thinking, visible)
}
```

**Usage:**
```rust
let (thinking, visible) = parse_llm_content(&llm_response.content);

// Log thinking (not sent to user)
if let Some(t) = thinking {
    dispatcher.dispatch(Frame::req("log:append", json!({
        "scope": scope,
        "kind": "thinking",
        "actor": format!("head/{}", self.head_id),
        "content": t,
    }))).await?;
}

// Send visible content to user
if let Some(v) = visible {
    dispatcher.dispatch(Frame::req("chat:message", json!({
        "scope": scope,
        "content": v,
        "reply_to": reply_to,
    })).with_actor(format!("head/{}", self.head_id))).await?;
}
```

---

### 11. Handler Changes (`src/server/handler.rs`)

**Remove Redirect handling:**

**Current (lines 272-303):**
```rust
FrameOp::Redirect => {
    // Convert to ChatChunk::ToolCall
}
```

**New:** Remove this branch. Tool calls come via `chat:tool` items.

**Update item handling (tool calls):**
```rust
FrameOp::Item => {
    if let Some(data) = frame.data {
        match data["type"].as_str() {
            Some("tool_call") => ChatChunk::ToolCall { ... },
            Some("text_delta") => ChatChunk::Delta(data["content"].as_str()?.to_string()),
            _ => continue,
        }
    }
}
```

---

### 12. OpenAI Handler Changes (`src/server/openai.rs`)

**Update response stream handling:**

Remove `FrameOp::Redirect` handling (lines 938-1003).

Add `FrameOp::Item` handling for tool calls:
```rust
FrameOp::Item => {
    if let Some(data) = &frame.data {
        if data["type"].as_str() == Some("tool_call") {
            // Emit OpenAI tool_call format
        }
    }
}
```

---

### 13. Web Chat Handler Changes (`src/server/web_chat.rs`)

**Update stream handling:**

**Current (lines 138-147):**
```rust
FrameOp::Redirect => {
    yield Event::default()
        .event("tool")
        .data(serde_json::to_string(&frame.data)?);
}
```

**New:**
```rust
FrameOp::Item => {
    if let Some(data) = &frame.data {
        if data["type"].as_str() == Some("tool_call") {
            yield Event::default()
                .event("tool")
                .data(serde_json::to_string(&data)?);
        }
    }
}
```

---

### 14. LlmClient Changes (`src/syscalls/llm.rs`)

**Emit structured items instead of a single ok response:**

**Current:** Returns single `Frame::ok` with full response

**New:** Emit `Frame::item` for each logical piece:

```rust
// Parse response
let content = response.content.as_deref().unwrap_or("");
let (thinking, visible) = parse_llm_content(content);

// Emit thinking item
if let Some(t) = thinking {
    tx.send(Frame::item(ctx.frame_id(), json!({
        "type": "thinking",
        "content": t,
    }))).await?;
}

// Emit text item
if let Some(v) = visible {
    tx.send(Frame::item(ctx.frame_id(), json!({
        "type": "text_delta",
        "content": v,
    }))).await?;
}

// Emit tool call items
for tc in response.tool_calls {
    tx.send(Frame::item(ctx.frame_id(), json!({
        "type": "tool_call",
        "tool_call_id": tc.id,
        "name": tc.function.name,
        "arguments": tc.function.arguments,
    }))).await?;
}

// Signal completion
tx.send(Frame::done(ctx.frame_id())).await?;
```

---

### 15. System Prompt Changes (`src/runtime/head_bundle.rs`)

**Update head system prompt to use thinking tags:**

**Add to Communication section:**
```
## Communication

Text in your response goes directly to the user as chat. Use tool calls for actions.

For internal reasoning that should NOT be shown to the user, wrap it in <thinking> tags:

<thinking>
I should check if the file exists before reading it...
</thinking>

Everything outside <thinking> tags is visible to the user.
```

---

### 16. Remove `head__chat_send` Tool (`src/agent_tools.rs`)

This tool is no longer needed. Heads emit `chat:message` syscalls directly.

**Remove:**
- `ChatSend` struct and implementation (around line 680-720)
- Registration in tool list

---

### 17. Router Lane Assignment (`src/kernel/router.rs`)

**Add chat syscalls to Immediate lane:**

```rust
fn route_syscall(name: &str) -> Lane {
    match name {
        // ... existing ...
        "chat:message" | "chat:tool" | "chat:tool_result" | "chat:done" | "chat:error" | "chat:cancel" => Lane::Immediate,
        // ...
    }
}
```

---

## Migration Steps

1. Create `src/syscalls/chat.rs` with `chat:message`, `chat:tool`, `chat:tool_result`, `chat:done`, `chat:error`, `chat:cancel`
2. Update `src/syscalls/mod.rs` to register chat module
3. Update `src/kernel/frame.rs` to remove `FrameOp::Redirect`
4. Update `src/kernel/router.rs` to route `chat:*` to Immediate lane
5. Update `src/syscalls/llm.rs` to emit structured `item` frames (`thinking`/`text`/`tool_call`) then `done`
6. Update `src/runtime/head_service.rs` to:
   - Consume `llm:chat` items (log thinking, forward text as `chat:message`)
   - Route internal tool calls to tasks
   - Batch external tool calls via `chat:tool` then end the segment with `chat:done`
   - Resume on `chat:tool_result`
7. Update `src/server/ingress_hub.rs` / `src/server/handler.rs`:
   - Submit user input via `chat:message`
   - Submit tool results via `chat:tool_result`
   - Stream responses from the turn stream until `chat:done`/`chat:error`
8. Update `src/server/openai.rs` / `src/server/web_chat.rs` to remove redirect handling and handle tool calls via `item`
9. Update `src/runtime/head_bundle.rs` system prompt to require `<thinking>` tags
10. Remove `head__chat_send` from `src/agent_tools.rs`
11. Update TUI monitor (out of scope here) to handle new frame types

---

## Testing Checklist

- [ ] User message creates `chat:message` with `actor="user"`
- [ ] Head response creates `chat:message` with `actor="head/<id>"`
- [ ] Thinking blocks in `<thinking>` tags are logged but not sent to chat
- [ ] Single external tool call emits `chat:tool` then `chat:done`
- [ ] Multiple external tool calls emit all `chat:tool` then `chat:done`
- [ ] Internal tool calls execute without closing connection
- [ ] Mixed internal+external: internal first, then external, then done
- [ ] Client disconnect triggers `chat:cancel`
- [ ] TUI monitor shows new frame types correctly
- [ ] OpenAI-compat `/v1/chat/completions` works with streaming
- [ ] Web chat `/api/chat` works with SSE

---

## Future Considerations

- **Streaming text**: Currently `chat:message` sends complete content. Could add streaming variant if needed.
- **Need abstraction**: May be removable if `chat:message` can directly trigger head processing.
- **Event cleanup**: Audit remaining `op=event` usage, migrate to explicit syscalls where appropriate.
