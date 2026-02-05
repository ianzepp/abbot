# Syscall Refactor Specification

## Overview

This document specifies a refactor of Abbot's frame/syscall architecture to establish consistency, clarity, and fix several design issues including:

- Inconsistent naming (`name` vs `data.kind`)
- Meta-commentary leaking into chat (thinking blocks shown to users)
- Single external tool limitation (redirect closes connection immediately)
- Unclear event semantics

## Design Principles

1. **`name` is always `<namespace>:<verb>`** - consistent, greppable, routable
2. **Actor indicates authorship** - `actor` field on frames, not buried in `data.kind`
3. **Explicit thinking separation** - LLM wraps thinking in `<thinking>` tags; bare text = chat
4. **Clean connection lifecycle** - `chat:done` signals completion, not abrupt redirect
5. **Multiple external tools** - emit all `chat:tool` frames before `chat:done`

---

## New Syscall Taxonomy

### Chat Namespace (`chat:*`)

| Syscall | Direction | Purpose |
|---------|-----------|---------|
| `chat:message` | Ingress/Egress | A chat message exists (from user or head) |
| `chat:tool` | Egress | External tool call for client to execute |
| `chat:done` | Egress | Head finished processing, connection can close |
| `chat:error` | Egress | Error occurred, connection should close |
| `chat:cancel` | Ingress | Client closed connection, cancel in-flight work |

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
    pub name: Option<String>,     // Always <namespace>:<verb> for Req
    pub parent_id: Option<Uuid>,  // Correlation to parent request
    pub actor: Option<String>,    // Who sent: "user", "head/<id>", "hand/<id>", "system"
    pub data: Option<Value>,      // Operation-specific payload
}
```

### Remove `FrameOp::Redirect`

The `Redirect` op is removed entirely. External tool calls use `chat:tool` syscall instead.

---

## Data Payloads

### `chat:message`

```json
{
  "scope": "main" | "session/<hash>",
  "content": "Hello, how can I help?",
  "reply_to": "<uuid>",           // Optional: correlates to original message
  "thinking": false               // Optional: true if this is internal reasoning
}
```

Actor field on frame indicates sender:
- `actor: "user"` - user sent this message
- `actor: "head/<id>"` - head sent this message

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

### `chat:done`

```json
{
  "scope": "main" | "session/<hash>",
  "reply_to": "<uuid>"
}
```

### `chat:cancel`

```json
{
  "scope": "main" | "session/<hash>",
  "reply_to": "<uuid>",
  "reason": "client_disconnect"
}
```

### `llm:chat` (unchanged, but response items clarified)

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
{ "type": "text", "content": "Hello!" }

// Thinking (extracted from <thinking> tags)
{ "type": "thinking", "content": "I should greet the user..." }

// Tool call
{ "type": "tool_call", "id": "...", "name": "...", "arguments": {...} }
```

Followed by `op=done` when complete.

---

## Request Flow

### User Sends Message

```
1. HTTP POST /v1/chat/completions
   ↓
2. IngressHub creates chat:message syscall
   Frame {
     op: Req,
     name: "chat:message",
     actor: "user",
     data: { scope: "main", content: "Hello" }
   }
   ↓
3. ChatHandler receives chat:message
   - Stores message to conversation log
   - Creates need:enqueue referencing the message
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
   - text → chat:message with actor="head/<id>"
   ↓
9. Head emits chat:done
   ↓
10. IngressHub sees chat:done, closes HTTP response
```

### With Internal Tool Calls

```
7. Response arrives:
   ← item { type: "text", content: "Let me check..." }
   ← item { type: "tool_call", name: "head__fs_read", ... }
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
   - chat:done
   ↓
10. IngressHub streams tool calls to client, closes connection
   ↓
11. Client executes tools, sends new request with results
   ↓
12. IngressHub creates chat:message with actor="user", type="tool_result"
   ↓
13. Head picks up, feeds to LLM, continues
```

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
   - Internal tool tasks can complete or be cancelled
   - No response sent (connection gone)
```

---

## Code Changes Required

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
        let reply_to = data.get("reply_to").and_then(|v| ...);
        let actor = ctx.actor(); // "user" or "head/<id>"

        // Log the message
        ctx.dispatcher().dispatch(Frame::req("log:append", json!({
            "scope": scope,
            "kind": "chat:message",
            "actor": actor,
            "content": content,
            "reply_to": reply_to,
        }))).await?;

        // If from head, forward to reply stream
        if actor.starts_with("head/") {
            if let Some(reply_to) = reply_to {
                ctx.sigcalls().send(scope, reply_to, Frame::bytes(
                    ctx.frame_id(),
                    json!({"text": content})
                ).with_name("chat:message"));
            }
        }

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

        // Send to reply stream (client will see this as a tool call)
        ctx.sigcalls().send(scope, reply_to, Frame::item(
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

### 4. New Syscall: `chat:done` (`src/syscalls/chat.rs`)

```rust
pub struct ChatDone;

impl Syscall for ChatDone {
    fn name(&self) -> &'static str { "chat:done" }

    async fn execute(&self, ctx: SyscallContext, data: Value, tx: FrameSender) -> Result<(), KernelError> {
        let scope = data["scope"].as_str().ok_or(...)?;
        let reply_to = Uuid::parse_str(data["reply_to"].as_str().ok_or(...)?)?;

        // Send done frame and close stream
        ctx.sigcalls().send(scope, reply_to, Frame::done(ctx.frame_id()));
        ctx.sigcalls().close(scope, reply_to);

        tx.send(Frame::ok(ctx.frame_id(), json!({"closed": true}))).await?;
        Ok(())
    }
}
```

---

### 5. New Syscall: `chat:cancel` (`src/syscalls/chat.rs`)

```rust
pub struct ChatCancel;

impl Syscall for ChatCancel {
    fn name(&self) -> &'static str { "chat:cancel" }

    async fn execute(&self, ctx: SyscallContext, data: Value, tx: FrameSender) -> Result<(), KernelError> {
        let scope = data["scope"].as_str().ok_or(...)?;
        let reply_to = Uuid::parse_str(data["reply_to"].as_str().ok_or(...)?)?;

        // Cancel any pending need for this reply_to
        // This will cause the head to stop processing
        ctx.dispatcher().dispatch(Frame::req("need:cancel", json!({
            "reply_to": reply_to,
        }))).await?;

        tx.send(Frame::ok(ctx.frame_id(), json!({"cancelled": true}))).await?;
        Ok(())
    }
}
```

---

### 6. Register Chat Syscalls (`src/syscalls/mod.rs`)

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
    dispatcher.register(Arc::new(ChatDone));
    dispatcher.register(Arc::new(ChatCancel));
}
```

---

### 7. IngressHub Changes (`src/server/ingress_hub.rs`)

**Current:** Creates `log:append` with `kind: "chat:user"`, then `need:enqueue`

**New:** Creates `chat:message` syscall which handles both logging and need creation

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
    "content": content,
})).with_actor("user");
```

**File:** `src/server/ingress_hub.rs`
- `submit_user_turn()` (line ~28-45)
- `submit_tool_results()` (line ~47-124) - similar changes for tool results

---

### 8. HeadService Changes (`src/runtime/head_service.rs`)

**Major changes needed:**

#### a. Replace redirect with chat:tool + chat:done

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

// After all internal processing complete, emit external calls
for tc in external_calls {
    dispatcher.dispatch(Frame::req("chat:tool", json!({
        "scope": scope,
        "reply_to": reply_to,
        "tool_call_id": tc.id,
        "name": tc.function.name,
        "arguments": tc.function.arguments,
    })).with_actor(format!("head/{}", self.head_id))).await?;
}

// Then close
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

**New logic after receiving LLM response:**
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

### 9. Handler Changes (`src/server/handler.rs`)

**Remove Redirect handling:**

**Current (lines 272-303):**
```rust
FrameOp::Redirect => {
    // Convert to ChatChunk::ToolCall
}
```

**New:** Remove this branch. Tool calls come via `chat:tool` items.

**Update item handling:**
```rust
FrameOp::Item => {
    if let Some(data) = frame.data {
        match data["type"].as_str() {
            Some("tool_call") => ChatChunk::ToolCall { ... },
            Some("text") => ChatChunk::Delta(data["content"].as_str()?.to_string()),
            _ => continue,
        }
    }
}
```

---

### 10. OpenAI Handler Changes (`src/server/openai.rs`)

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

### 11. Web Chat Handler Changes (`src/server/web_chat.rs`)

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

### 12. LlmClient Changes (`src/syscalls/llm.rs`)

**Emit structured items instead of raw response:**

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
        "type": "text",
        "content": v,
    }))).await?;
}

// Emit tool call items
for tc in response.tool_calls {
    tx.send(Frame::item(ctx.frame_id(), json!({
        "type": "tool_call",
        "id": tc.id,
        "name": tc.function.name,
        "arguments": tc.function.arguments,
    }))).await?;
}

// Signal completion
tx.send(Frame::done(ctx.frame_id())).await?;
```

---

### 13. System Prompt Changes (`src/runtime/head_bundle.rs`)

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

### 14. Remove `head__chat_send` Tool (`src/agent_tools.rs`)

This tool is no longer needed. Heads emit `chat:message` syscalls directly.

**Remove:**
- `ChatSend` struct and implementation (around line 680-720)
- Registration in tool list

---

### 15. Router Lane Assignment (`src/kernel/router.rs`)

**Add chat syscalls to Immediate lane:**

```rust
fn route_syscall(name: &str) -> Lane {
    match name {
        // ... existing ...
        "chat:message" | "chat:tool" | "chat:done" | "chat:cancel" => Lane::Immediate,
        // ...
    }
}
```

---

## Migration Steps

1. **Create `src/syscalls/chat.rs`** with new syscalls
2. **Update `src/syscalls/mod.rs`** to register chat module
3. **Update `src/kernel/frame.rs`** to remove `Redirect` op
4. **Update `src/kernel/router.rs`** for lane assignment
5. **Update `src/syscalls/llm.rs`** to emit structured items
6. **Update `src/runtime/head_service.rs`**:
   - Parse thinking tags
   - Use `chat:message` instead of direct sigcall send
   - Use `chat:tool` + `chat:done` instead of redirect
7. **Update `src/server/ingress_hub.rs`** to use `chat:message`
8. **Update `src/server/handler.rs`** to handle new item types
9. **Update `src/server/openai.rs`** to remove redirect handling
10. **Update `src/server/web_chat.rs`** to remove redirect handling
11. **Update `src/runtime/head_bundle.rs`** system prompt
12. **Remove `head__chat_send`** from `src/agent_tools.rs`
13. **Update TUI** (`tui/src/main.rs`) to handle new frame types in monitor

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
