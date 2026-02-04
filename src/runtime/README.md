# Runtime

The runtime implements a Mind/Head/Hand architecture with pooled workers and queue-based dispatching.

## Architecture

```
                         ┌─────────────┐
                         │   Human     │
                         │  (CLI/API)  │
                         └──────┬──────┘
                                │ chat message
                                ▼
┌──────────────────────────────────────────────────────────────────────┐
│                          RuntimeBus                                   │
│              (pub/sub message passing + sqlite persistence)           │
└──────────────────────────────────────────────────────────────────────┘
        │               │                │                │
        ▼               ▼                ▼                ▼
┌───────────┐    ┌─────────────┐   ┌─────────────┐   ┌─────────────┐
│   Mind    │    │ NeedService │   │ GoalService │   │    Hand     │
│(Conclave) │    │             │   │             │   │   (pool)    │
├───────────┤    ├─────────────┤   ├─────────────┤   ├─────────────┤
│Mind/Head/ │    │priority queue│   │FIFO + RR    │   │execute tools│
│deliberate │    │head pool (3) │   │hand pool (4)│   │return result│
│on ticks   │    │dispatch needs│   │dispatch goal│   │             │
└─────┬─────┘    └──────┬──────┘   └──────┬──────┘   └─────────────┘
      │                 │                 │
      │ need            │ wake head       │ assign goal
      ▼                 ▼                 ▼
                 ┌─────────────┐
                 │    Head     │
                 │   (pool)    │
                 ├─────────────┤
                 │purely react │
                 │need → goal  │
                 │chat w/users │
                 └─────────────┘
```

## Components

### MindService (`mind_service.rs`)

Strategic layer. Triggers deliberation meetings on idle events.

- **Autonomy** (5 min idle): Operational retro - what happened, what's next?
- **Conclave** (1 hour idle): Strategic reflection - who are we, how should we grow?

MindManager, HeadManager, HandManager discuss and vote on proposals.

### Conclave (`conclave.rs`)

Deliberation loop for both Autonomy and Conclave meetings.

1. Build context (recent activity, LTM, wants pool)
2. Each Mind responds with thoughts, proposals, votes
3. Iterate until consensus or max rounds (5)
4. Proposals with 2/3 votes are executed

**Autonomy output:** Needs (what to do next), Wants (deferred work)
**Conclave output:** Self updates (identity), LTM updates (memory), strategic Wants

### Room (`room.rs`)

Data structures for deliberation:

- `Room` - deliberation space with transcript and decision
- `MindPersona` - MindManager/HeadManager/HandManager with role, model, temperature, system prompt
- `RoomDecision` - agreed needs, wants, LTM operations
- `NeedProposal`, `WantProposal` - proposals with votes

### NeedService (`need_service.rs`)

Dispatches needs to available heads. Priority queue ordering.

**Queue behavior:**
- Needs sorted by priority (urgent > high > normal > low)
- Within same priority, FIFO ordering
- Dispatches to first available head in pool

**Head pool:**
- Default 3 heads (`head-0`, `head-1`, `head-2`)
- States: `Available` or `Processing { need_id, started_at }`
- 10 minute timeout per need (configurable)

**Message flow:**
- Receives `NeedMsg::Request` → enqueue
- Receives `NeedMsg::Fulfilled` → mark head available, notify mind

### HeadService (`head_service.rs`)

Tactical layer. Purely reactive - only wakes on incoming messages.

**Triggers:**
- Chat messages in watched scopes
- Need assignments from NeedService
- Goal results from GoalService

**Responsibilities:**
1. Process needs and user messages
2. Create goals for work that needs doing
3. Respond to users via chat
4. Track outstanding goals per scope

**No scheduled wakes** - head only acts when triggered by events.

### GoalService (`goal_service.rs`)

Dispatches goals to available hands. FIFO with round-robin by scope.

**Queue behavior:**
- Separate FIFO queue per notify_scope (e.g., `#general`, `@alice`)
- Round-robin across scopes for fairness
- Empty queues are removed from rotation

**Hand pool:**
- Default 4 hands (`hand-0`, `hand-1`, `hand-2`, `hand-3`)
- States: `Idle` or `Running { task_id, goal_id, head_id, started_at }`
- 5 minute timeout per goal (configurable)

**Message flow:**
- Receives `TaskMsg::Request` → enqueue goal
- Receives `TaskMsg::Result` → mark hand idle, notify head
- Sends `goals_drained` event when all goals for a scope complete

### HandService (`hand_service.rs`)

Operational layer. Executes goals using tools.

**Execution loop:**
1. Receive `TaskAssigned` message
2. Build conversation via `HandBundleBuilder`
3. Call LLM with tool definitions from `agent_tools.rs`
4. LLM returns tool_calls in response
5. Execute tools via `exec_hand_tool()`
6. Repeat until final content or iteration limit (24)
7. Publish `TaskMsg::Result`

**Failure handling:**
- Tool errors don't immediately fail the task
- Hand can retry up to 5 consecutive tool failures
- No action for 3 iterations fails the task
- Iteration limit (24) fails the task

## Message Flow Example

1. User sends: "count the rust files"
2. Message becomes Need (normal priority) → NeedService
3. NeedService dispatches to `head-0` (available)
4. HeadService receives, calls LLM, creates Goal "count rust files"
5. Goal → GoalService queue
6. GoalService dispatches to `hand-0` (idle)
7. HandService executes: `rg --files -g "*.rs" | wc -l`
8. Hand returns `<result ok="true">Found 56 Rust files.</result>`
9. GoalService notifies HeadService of completion
10. HeadService responds to user with result

## Head Grammar

Head communicates via structured blocks:

```
--- chat #channel ---
message content
--- end ---

--- mail @recipient ---
message content
--- end ---

--- goal ---
count the rust files in the project
--- end ---
```

## Hand Grammar

Hand responds with tool calls or results:

```
<exec tool="bash">
rg --files -g "*.rs" | wc -l
</exec>

<result ok="true">
Found 56 Rust files.
</result>

<result ok="false">
Could not find the requested file.
</result>
```

## Mind Grammar (Autonomy/Conclave)

Minds respond with JSON:

```json
{
  "thoughts": "The user seems focused on code quality...",
  "proposals": [
    { "type": "need", "text": "Review recent changes", "priority": "normal" }
  ],
  "votes": {
    "need:Run tests": "yes",
    "want:Document API": "no"
  },
  "consensus": false
}
```

## Configuration

Runtime config is file-based (no `HEAD_*`/`HAND_*`/`MIND_*` env overrides).

- Global config: `~/.config/abbot/abbot.toml`
- Workspace config (LLM-writable): `<workspace>/config.toml`

Example:

```toml
[providers.openai]
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[pool]
size = 4          # hand pool size
timeout_secs = 300  # goal timeout

[head]
model = "openai/gpt-5.2"
debounce_ms = 500

[hand]
model = "openai/gpt-5.2"
max_iters = 24

[mind]
model = "openai/gpt-5.2"
tick_interval = 60
```

## Tools

Available to hands via `agent_tools.rs` (JSON tool_calls):

| Tool | Purpose |
|------|---------|
| bash | Run shell commands |
| read | Read file contents |
| write | Create/overwrite files |
| edit | Modify files (search/replace) |
| find | Find files by pattern |
| diff | Compare files or git state |
| patch | Apply unified diffs |
| cd | Change working directory |
| recall | Search indexed transcripts |

## Files

```
runtime/
├── mod.rs              # exports
├── config.rs           # unified config loading
├── app_config.rs       # app-wide config (pool, etc.)
├── process_state.rs    # process-level config (bind addr)
│
├── mind_service.rs     # conclave coordinator
├── mind_bundle.rs      # mind conversation builder
├── mind_config.rs      # mind env config
├── mind_parser.rs      # mind response parser
├── mind_system.md      # mind identity
├── mind_grammar.md     # mind response format
├── mind_manager.md     # MindManager persona prompt
├── head_manager.md     # HeadManager persona prompt
├── hand_manager.md     # HandManager persona prompt
│
├── room.rs             # Room structures for deliberation
├── room_autonomy.md    # autonomy meeting response format
├── room_conclave.md    # conclave meeting response format
├── conclave.rs         # deliberation loop (both meeting types)
│
├── kernel.rs           # kernel facade (syscalls/streams)
│
├── head_service.rs     # head: reactive decision maker
├── head_bundle.rs      # head conversation builder
├── head_config.rs      # head env config
├── head_parser.rs      # head response parser
├── head_system.md      # head identity
├── head_grammar.md     # head response format
│
├── hand_service.rs     # hand: tool executor
├── hand_bundle.rs      # hand conversation builder
├── hand_config.rs      # hand env config
├── hand_parser.rs      # hand response parser
├── hand_allocator.rs   # hand slot tracking
├── hand_system.md      # hand identity
├── hand_grammar.md     # hand response format
│
├── llm_harness.rs      # LLM call wrapper
├── parser.rs           # shared parsing utilities
└── exec.rs             # direct tool execution
```

## Logging

Set `RUST_LOG=info` to see the event flow:

| Event | Log Message |
|-------|-------------|
| Autonomy convening | `slow idle reached; convening autonomy` |
| Conclave convening | `deep idle reached; convening conclave` |
| Meeting consensus | `autonomy/conclave reached consensus` |
| Need queued | `need queued need_id=... source=... priority=...` |
| Need dispatched | `dispatching need to head need_id=... head_id=...` |
| Need fulfilled | `need fulfilled need_id=... head_id=...` |
| Goal queued | `goal queued task_id=... head_id=...` |
| Goal dispatched | `dispatching goal to hand task_id=... hand_id=...` |
| Goal completed | `goal completed task_id=... hand_id=... ok=...` |
| Tool executed | `tool executed hand_id=... tool=... success=...` |
