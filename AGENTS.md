# Abbot - Agent Guide

This document is for AI agents (including Abbot's own Mind/Head/Hand) working on or with this codebase.

## What Is Abbot?

Abbot is a message-first microkernel daemon implementing distributed AI cognition. It runs as:
- **OpenAI-compatible provider** - Clients (like Opencode CLI) talk to Abbot via `/v1/chat/completions`
- **Intelligent proxy** - Abbot sits between upstream LLM providers and clients, rewriting prompts, routing tools
- **Autonomous system** - Proactive Mind layer runs on SIGTICK to handle background work

## Core Architecture

### Kernel-Driven Services

All communication flows through `KernelDispatcher` via **Frames**:

```rust
Frame {
    id: Uuid,              // Request/response correlation
    op: FrameOp,           // Req | Ok | Done | Error | Item | Progress | Cancel | Redirect
    name: Option<String>,  // Syscall name (e.g., "need:enqueue")
    parent_id: Option<Uuid>, // Links responses to requests
    actor: Option<String>, // Scope/session identifier
    data: Option<Value>,   // Payload (JSON)
}
```

**Key principle**: Services never call each other directly. All interaction is via syscalls.

### Syscall Namespaces

| Namespace | Purpose | Key Operations |
|-----------|---------|----------------|
| `need:*` | Work queue for Head | `enqueue`, `lease`, `ack`, `fulfill` |
| `task:*` | Work queue for Hand | `enqueue`, `lease`, `progress`, `result`, `cancel` |
| `room:*` | Deliberation rooms | `create`, `join`, `propose`, `vote`, `close` |
| `tick:*` | Timer signals | `subscribe`, `unsubscribe` |
| `log:*` | Audit/query | `append`, `select` |
| `reply:*` | Reply streams | `send`, `close` |

### Mind / Head / Hand

```
Mind (strategic)           Head (tactical)            Hand (operational)
─────────────────          ──────────────             ─────────────────
- Subscribes to SIGTICK    - Leases needs             - Leases tasks
- Creates needs            - Converts needs→tasks     - Executes tools via HAL
- Manages LTM + Self       - Manages STM              - Returns results as frames
- Deliberates in Conclave  - Responds to users        - No LLM calls
```

**Flow**:
1. User message → `need:enqueue` (priority: normal)
2. Head polls `need:lease` → acquires need
3. Head processes, calls `task:enqueue` for work
4. Hand polls `task:lease` → executes tools → streams `Item`/`Ok`/`Done` frames
5. Head receives task results → responds to user via `reply:send`

## Memory Model

### Self (Collective Identity)

Who we are as a system. Managed by Conclave via proposals:
- **Operations**: `append`, `replace`, `remove`
- **Consensus**: Requires 2/3 vote from MindManager/HeadManager/HandManager
- **Scope**: Injected into all agent contexts

### LTM (Long-Term Memory)

Strategic, persistent learnings. Managed by Conclave:
- **Operations**: `append`, `replace`, `remove`
- **Consensus**: Requires 2/3 vote
- **Scope**: Flows automatically into Head contexts

### STM (Short-Term Memory)

Tactical, working memory. Managed by Head via tools:
- **Operations**: `read_stm`, `update_stm` (set/append/clear)
- **Scope**: Flows automatically into Hand contexts when tasks are created
- **Purpose**: Track conversation state, user preferences, in-progress work

## EMS, VFS, HAL

### EMS (Entity Management System)

Schema-flexible SQLite entity store (`src/ems/`):
- **Schema-on-write**: Tables/columns created lazily
- **All TEXT columns**: JSON encoding for nested structures
- **Operations**: `insert`, `update`, `delete`, `select`, `query`
- **Used for**: Wants pool, conversation history, task state

```rust
// Example: Query wants pool
ems.select("wants", r#"{"status": "pending"}"#, None, None)?;
```

### VFS (Virtual File System)

Mount-based filesystem isolation (`src/vfs/`):
- **Security model**: No access without mounts
- **Resolution**: Longest-prefix match wins
- **Read-only enforcement**: Mounts can be marked `ro`
- **Symlink escapes**: Logged as warnings, not blocked

All filesystem syscalls resolve guest paths through `MountTable::resolve()` before accessing host filesystem.

### HAL (Hardware Abstraction Layer)

Abstraction over host operations (`src/hal/`):
- **HalFs**: File operations (via VFS)
- **HalGit**: Git command execution
- **HalNet**: HTTP requests (curl)
- **HalProcess**: Process spawning

Hand tools call HAL interfaces, never touch host filesystem directly.

## Frame Protocol

### Terminal vs Non-Terminal

**Terminal frames** (end the stream):
- `Ok`: Single-value success
- `Done`: Stream ended successfully (no more items)
- `Error`: Failure

**Non-terminal frames** (more frames may follow):
- `Item`: Stream item
- `Bytes`: Binary chunk
- `Event`: Event notification
- `Progress`: Progress update

### Backpressure

Kernel maintains per-stream buffers with watermarks:
- Producer pauses at high-water mark
- Consumer drains frames
- Producer resumes at low-water mark
- Consumer can `Cancel` any time
- Kernel aborts stalled streams (no drain activity for timeout)

### External Tools (Sigcall)

External tools (Opencode CLI) are routed via `Redirect` frames:
1. Head calls tool → Kernel checks if external
2. Kernel emits `Redirect` frame → transported via OpenAI `tool_call`
3. Opencode executes tool → returns result via OpenAI tool result
4. Kernel receives result → continues stream

This is the "sigcall" pattern (reverse syscall) from the monkify design.

## Development Patterns

### Adding a Syscall

1. Define syscall in appropriate namespace module (`src/kernel/needs.rs`, etc.)
2. Implement `Syscall` trait:
```rust
impl Syscall for MyKernel {
    fn call(&self, ctx: &SyscallContext, req: &Frame) -> Result<SyscallResponse>;
}
```
3. Register in `KernelRouter` dispatch table
4. Add to syscall namespace table in README.md

### Adding a Tool (Head or Hand)

1. Define tool spec in bundle (`head_bundle.rs` or `hand_bundle.rs`)
2. Implement execution in tool handler
3. For Hand tools: Use HAL interfaces, never direct filesystem access
4. For Head tools: Keep read-only, bounded (e.g., max 100 lines for `read_file`)
5. Update README.md tools table

### Adding EMS Entity Type

No schema definition needed - just insert:
```rust
ems.insert("entity_type", json!({
    "id": uuid,
    "field1": "value",
    "nested": {"foo": "bar"}
}))?;
```

EMS automatically creates `entity_type` table and adds columns as needed.

### Testing

- **Unit tests**: `cargo test`
- **Integration tests**: `tests/` directory
- **Kernel stress tests**: `tests/kernel_dispatch_stress_test.rs`
- **Manual testing**: `cargo run -- --prompt "test message" --exit`

### Logging

Structured logging via `tracing`:
- `info!`: Flow + decisions (default)
- `debug!`: Internal details
- `trace!`: Frame-level messages

Set `RUST_LOG=info` or `RUST_LOG=debug` to control verbosity.

## Constraints & Conventions

### Services MUST NOT:
- Call other services directly (use syscalls)
- Access filesystem outside VFS mounts
- Block indefinitely (respect deadlines)
- Mutate shared state without syscalls

### Frames MUST:
- Include `parent_id` for responses
- Use appropriate `op` (terminal vs non-terminal)
- Serialize `data` as JSON `Value`

### Syscalls MUST:
- Validate inputs (reject malformed requests with `Error` frame)
- Stream large results (`Item` frames, then `Done`)
- Respect backpressure (don't flood receivers)
- Support `Cancel` gracefully

### Tools MUST:
- Validate paths via VFS (Hand tools)
- Enforce limits (e.g., max results, offset+limit)
- Return structured results (JSON)
- Use HAL interfaces, not raw `std::fs`

## Key Files

**Kernel**:
- `src/kernel/dispatcher.rs` - Frame routing, backpressure
- `src/kernel/frame.rs` - Frame protocol types
- `src/kernel/router.rs` - Syscall dispatch

**Runtime**:
- `src/runtime/kernel.rs` - Kernel harness
- `src/runtime/mind_service.rs` - Mind implementation
- `src/runtime/head_service.rs` - Head implementation
- `src/runtime/hand_service.rs` - Hand implementation
- `src/runtime/conclave.rs` - Deliberation loop

**Layers**:
- `src/ems/service.rs` - EMS implementation
- `src/vfs/mount.rs` - VFS mount resolution
- `src/hal/fs.rs` - Filesystem HAL
- `src/hal/git.rs` - Git HAL
- `src/hal/net.rs` - HTTP HAL

**Server**:
- `src/server/handler.rs` - OpenAI `/v1/chat/completions` endpoint
- `src/server/anthropic.rs` - Anthropic compatibility layer

## Philosophy

From `docs/monkify-overview.md`:

> Recast Abbot as a small "kernel" that brokers all work as message streams, and as an intelligent proxy:
>
> `OpenAI Provider` <-> `Abbot` <-> `Opencode CLI`

**Message-first**: The fundamental unit is not "function returns value" but `Message -> AsyncIterable<Response>`.

**Backpressure as protocol**: Streams are consumer-driven. Producer pauses when buffer fills, resumes when consumer drains.

**Hard boundary**: Services communicate only via kernel-managed channels, never by reaching into shared state.

**Sigcall**: External tools (Opencode) are routed through kernel as first-class `Redirect` frames, turning the ad-hoc `external_tool_request` pattern into a kernel facility.

## Common Tasks

### Query recent conversation history
```rust
// Use log:select syscall
let result = kernel.call("log:select", json!({
    "actor": "session/abc123",
    "limit": 20
}))?;
```

### Create a need
```rust
// Use need:enqueue syscall
kernel.call("need:enqueue", json!({
    "actor": "session/abc123",
    "priority": "normal",
    "instruction": "Analyze the codebase",
}))?;
```

### Create a task
```rust
// Use task:enqueue syscall
kernel.call("task:enqueue", json!({
    "actor": "session/abc123",
    "instruction": "Read and summarize README.md",
    "context": {"stm": "..."}, // STM flows into Hand
}))?;
```

### Subscribe to SIGTICK
```rust
// Mind subscribes on startup
kernel.call("tick:subscribe", json!({}))?;
// Kernel broadcasts tick frames periodically
```

### Create deliberation room
```rust
// Mind creates room for Conclave
let room_id = kernel.call("room:create", json!({
    "kind": "conclave",
    "participants": ["mind_manager", "head_manager", "hand_manager"]
}))?;
```

---

**Remember**: Abbot is a microkernel. Services are userspace processes. Communication is via syscalls. Files go through VFS. Hardware goes through HAL. Messages flow as streams. Backpressure is protocol.
