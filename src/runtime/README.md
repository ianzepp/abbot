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
│(Boardroom)│    │             │   │             │   │   (pool)    │
├───────────┤    ├─────────────┤   ├─────────────┤   ├─────────────┤
│CEO/CTO/CFO│    │priority queue│   │FIFO + RR    │   │execute tools│
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

Strategic layer. Convenes the Boardroom on heartbeat ticks.

- Wakes on `Ping` messages at configured tick interval
- Creates a Conclave to run deliberation
- CEO, CTO, CFO personas discuss and vote on proposals
- Output: Needs (immediate action) and Wants (aspirational)

### Conclave (`conclave.rs`)

Deliberation loop for the Boardroom.

1. Build context (recent activity, LTM, wants pool)
2. Each Mind responds with thoughts, proposals, votes
3. Iterate until consensus or max rounds (5)
4. Proposals with 2/3 votes are executed
5. Needs → NeedService queue; Wants → SQLite wants pool

### Room (`room.rs`)

Data structures for deliberation:

- `Room` - deliberation space with transcript and decision
- `MindPersona` - CEO/CTO/CFO with role, model, temperature, system prompt
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
3. Call LLM
4. Parse response for tool calls or result
5. Execute tools via `Dispatcher`
6. Repeat until `<result>` or iteration limit (24)
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

## Mind Grammar (Boardroom)

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

### Mind Config

| Env Var | Purpose | Default |
|---------|---------|---------|
| `MIND_MODEL` | Model for mind personas | claude-3-haiku |
| `MIND_API_KEY` | API key | - |
| `MIND_BASE_URL` | API endpoint | anthropic |
| `MIND_TICK` | Deliberate every N ticks (0=disabled) | 60 |

### Head Config

| Env Var | Purpose | Default |
|---------|---------|---------|
| `HEAD_MODEL` | Model name | - |
| `HEAD_API_KEY` | API key | - |
| `HEAD_BASE_URL` | API endpoint | OpenAI |
| `HEAD_TEMPERATURE` | Sampling temperature | 0.7 |
| `HEAD_MAX_TOKENS` | Max response tokens | - |
| `HEAD_DEBOUNCE_MS` | Debounce before thinking | 500 |

### Hand Config

| Env Var | Purpose | Default |
|---------|---------|---------|
| `HAND_MODEL` | Model name | - |
| `HAND_API_KEY` | API key | - |
| `HAND_BASE_URL` | API endpoint | OpenAI |
| `HAND_TEMPERATURE` | Sampling temperature | 0.2 |
| `HAND_MAX_TOKENS` | Max response tokens | - |
| `HAND_MAX_ITERS` | Max tool iterations | 24 |

### Pool Config (config.toml)

```toml
[pool]
size = 4          # hand pool size
timeout_secs = 300  # goal timeout
```

## Tools

Available to hands via `Dispatcher`:

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
├── bus.rs              # RuntimeBus wrapper
├── config.rs           # unified config loading
├── app_config.rs       # app-wide config (pool, etc.)
├── models_config.rs    # models.toml parsing
│
├── mind_service.rs     # boardroom coordinator
├── mind_bundle.rs      # mind conversation builder
├── mind_config.rs      # mind env config
├── mind_parser.rs      # mind response parser
├── mind_system.md      # mind identity
├── mind_grammar.md     # mind response format
├── mind_ceo.md         # CEO persona prompt
├── mind_cto.md         # CTO persona prompt
├── mind_cfo.md         # CFO persona prompt
│
├── room.rs             # Room/Boardroom structures
├── room_grammar.md     # room response format
├── conclave.rs         # deliberation loop
│
├── need_service.rs     # need dispatcher (priority queue)
├── goal_service.rs     # goal dispatcher (FIFO + RR)
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
| Boardroom convening | `boardroom convening tick=N` |
| Boardroom consensus | `boardroom reached consensus` |
| Need queued | `need queued need_id=... source=... priority=...` |
| Need dispatched | `dispatching need to head need_id=... head_id=...` |
| Need fulfilled | `need fulfilled need_id=... head_id=...` |
| Goal queued | `goal queued task_id=... head_id=...` |
| Goal dispatched | `dispatching goal to hand task_id=... hand_id=...` |
| Goal completed | `goal completed task_id=... hand_id=... ok=...` |
| Tool executed | `tool executed hand_id=... tool=... success=...` |
