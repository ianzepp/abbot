# Implementation Plan

## Current State

**Done:**
- [x] Vision document (`VISION.md`)
- [x] 4-layer context model (`src/monk/context.rs`)
- [x] DB schema for monk_self, monk_workspace (`src/history/store.rs`)
- [x] Registry for monks and subscriptions (`src/monk/registry.rs`)
- [x] Runner for dispatching messages (`src/monk/runner.rs`)
- [x] Heartbeat task sending pings to `#ping`
- [x] Grammar specification (`monastery/grammar.md`)
- [x] System prompt with commandments (`monastery/system.md`)
- [x] Core infrastructure: pub/sub, IRC, basic tools
- [x] Parser for LLM responses (`src/monk/parser.rs`)
- [x] New tools: monk, channel, self, workspace (`src/tools/*.rs`)
- [x] Action executor (`src/monk/executor.rs`)
- [x] LLM integration in Monk::on_message()
- [x] Result feeding (tool results back to monk)

**Not Done:**
- [ ] End-to-end testing (manual testing with IRC client)

---

## Phase 1: Parser

Create a parser that extracts actions from LLM responses.

**File:** `src/monk/parser.rs`

**Input:** Raw LLM response text

**Output:**
```rust
struct ParsedResponse {
    actions: Vec<Action>,
    has_pong: bool,
}

enum Action {
    Say { channel: String, text: String },
    Exec { tool: String, content: String },
}
```

**Tasks:**
1. Parse `<say channel="...">...</say>` tags
2. Parse `<exec tool="...">...</exec>` tags
3. Parse `<pong/>` tag
4. Ignore text outside tags (thoughts)
5. Handle malformed responses gracefully
6. Unit tests for each case

---

## Phase 2: New Tools

Implement the new tools that don't exist yet.

### 2a: monk tool

**File:** `src/tools/monk.rs` (recreate from backup, modify)

**Commands:**
- `recruit name=X model=Y` → create monk, register, subscribe to #general and #ping
- `dismiss name=X` → unsubscribe, remove from registry, call monk.dismiss()
- `list` → return list of active monks

**Depends on:** Registry access from tool context

### 2b: channel tool

**File:** `src/tools/channel.rs` (new)

**Commands:**
- `join #channel` → subscribe monk to channel, switch context
- `part #channel` → unsubscribe monk from channel
- `list` → return channels monk is subscribed to

**Depends on:** Registry access, monk ID in execution context

### 2c: self tool

**File:** `src/tools/self.rs` (new)

**Commands:**
- `read` → return monk's layer 2 content
- `write\nCONTENT` → replace monk's layer 2
- `meditate` → trigger compaction (future: call LLM to summarize)

**Depends on:** Monk reference or store + monk_id in context

### 2d: workspace tool

**File:** `src/tools/workspace.rs` (new)

**Commands:**
- `read` → return monk's layer 3 for current channel
- `write\nCONTENT` → replace monk's layer 3 for current channel

**Depends on:** Monk reference or store + monk_id + channel in context

### 2e: Update ExecutionContext

Current:
```rust
pub struct ExecutionContext {
    pub cwd: PathBuf,
    pub sender: String,
    pub channel: String,
}
```

Needs:
```rust
pub struct ExecutionContext {
    pub cwd: PathBuf,
    pub sender: String,      // monk_id
    pub channel: String,
    pub store: Arc<Store>,
    pub registry: SharedRegistry,
}
```

---

## Phase 3: Action Executor

Execute parsed actions and collect results.

**File:** `src/monk/executor.rs`

**Input:** `ParsedResponse`, `ExecutionContext`

**Output:**
```rust
struct ExecutionResult {
    tool_results: Vec<ToolResult>,
    messages_sent: Vec<(String, String)>,  // (channel, text)
}

struct ToolResult {
    tool: String,
    content: String,
    output: String,
    success: bool,
}
```

**Tasks:**
1. Execute all `Exec` actions in parallel (tokio::spawn)
2. Execute all `Say` actions (publish to channels)
3. Collect results
4. Handle tool errors gracefully

---

## Phase 4: LLM Integration

Wire the LLM into `Monk::on_message()`.

**File:** `src/monk/context.rs` (update on_message)

**Flow:**
1. Build context (layers 1-4)
2. Format incoming message
3. Call LLM API with system prompt + user message
4. Parse response
5. Execute actions
6. If tool results exist, feed back to LLM (loop)
7. Limit iterations (prevent infinite loops)

**Tasks:**
1. Add LLM client to Monk struct
2. Implement the call → parse → execute loop
3. Format tool results for next LLM call
4. Add iteration limit (e.g., 5)
5. Handle LLM errors

---

## Phase 5: Result Feeding

When tools execute, feed results back to LLM for next iteration.

**Format for tool results:**
```xml
<tool_result tool="read" success="true">
file contents here...
</tool_result>

<tool_result tool="bash" success="false">
error: command not found
</tool_result>
```

**Tasks:**
1. Define result format in grammar.md (for LLM to understand)
2. Format results in executor
3. Append to user message for next LLM call

---

## Phase 6: Integration Testing

Test the full flow end-to-end.

**Tests:**
1. Ping → pong (no action)
2. Ping → read file → say result
3. Chat → respond with say
4. Recruit monk → verify in registry
5. Join channel → verify subscription
6. Self write → verify in DB
7. Multi-action parallel execution
8. Tool error handling
9. Iteration limit

---

## Dependency Order

```
Phase 1 (Parser)
    ↓
Phase 2e (ExecutionContext update)
    ↓
Phase 2a-2d (New tools) ←──────┐
    ↓                          │
Phase 3 (Executor)             │
    ↓                          │
Phase 4 (LLM Integration) ─────┘
    ↓
Phase 5 (Result Feeding)
    ↓
Phase 6 (Testing)
```

---

## Estimated Scope

| Phase | Files | Complexity |
|-------|-------|------------|
| 1. Parser | 1 new | Medium |
| 2. Tools | 4 new, 1 update | Medium |
| 3. Executor | 1 new | Medium |
| 4. LLM Integration | 1 update | High |
| 5. Result Feeding | 1 update | Low |
| 6. Testing | 1 new | Medium |

---

## Open Questions

1. **Iteration limit:** How many LLM calls per message? (Propose: 5)
2. **Tool timeout:** Per-tool or global? (Propose: per-tool, default 30s)
3. **Meditate implementation:** What does compaction look like? (Defer to later)
4. **Monk model selection:** How to specify model when recruiting? (haiku/sonnet/opus)
5. **Error visibility:** Do monks see tool errors in their context? (Propose: yes)
