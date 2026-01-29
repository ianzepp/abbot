# Runtime

The runtime implements a head/hand architecture for task execution, modeled on an octopus: the head is the will, the hands are the means.

## Architecture

```
                    +-------------+
                    |   Human     |
                    |  (CLI/IRC)  |
                    +------+------+
                           | chat message
                           v
+------------------------------------------------------+
|                     RuntimeBus                        |
|  (pub/sub message passing + sqlite persistence)       |
+------------------------------------------------------+
        |                                      |
        v                                      v
+------------------+                  +------------------+
|   HeadService    |   TaskAssigned   |   HandService    |
|                  | ---------------> |                  |
| - watches #chan  |                  | - executes tasks |
| - thinks via LLM |   TaskResult     | - runs tools     |
| - owns hand slots| <--------------- | - returns result |
| - delegates work |                  |                  |
+------------------+                  +------------------+
```

## Head/Hand Model

**Head** - The orchestrator. Watches channels, responds to humans, delegates work to hands. Does not execute tools directly.

**Hands** - The workers. Each head owns a fixed number of hand slots (default: 2). Hands execute goals using tools and return results.

### Hand Slot States

```
idle -> running -> success -> idle
                -> failed  -> idle
```

- **idle** - Available for new work
- **running** - Currently executing a task
- **success** - Task completed, awaiting head acknowledgment (`clear N`)
- **failed** - Task failed, awaiting head acknowledgment (`clear N`)

## Components

### RuntimeBus (`bus.rs`)

Central message passing. Wraps the pub/sub Hub and sqlite Store. All components publish and subscribe through the bus.

### HeadService (`head_service.rs`)

The orchestrator. Responsibilities:

1. **Watch channels** - Subscribes to configured scopes (e.g., `#general`)
2. **React to triggers** - Human messages, task completions, heartbeat
3. **Think via LLM** - Builds conversation context, calls LLM, parses response
4. **Manage hand slots** - Owns N hand slots, assigns tasks, tracks state
5. **Execute actions** - `chat`, `mail`, `hand` commands from LLM response

Key methods:
- `trigger()` - Determines if head should think based on incoming message
- `think()` - Calls LLM, parses response, executes actions
- `execute_goal()` - Assigns a goal to an available hand slot
- `execute_hand()` - Processes `list`, `goal`, `read`, `clear` commands

### HandService (`hand_service.rs`)

The worker. When a task is assigned:

1. Builds conversation via `HandBundleBuilder`
2. Calls LLM
3. Parses response for `<exec>` or `<result>` blocks
4. Executes tools via `Dispatcher`
5. Logs results to sqlite
6. Repeats until `<result>` or iteration limit
7. Publishes `TaskResult` (success or failure)

The hand doesn't decide what to do - it executes the head's intent using available tools.

### HeadBundleBuilder (`head_bundle.rs`)

Assembles the LLM conversation for a head:

- System message: `head_system.md` (identity/conduct) + `head_grammar.md` (response format)
- Channel messages with role assignment:
  - **assistant** - Messages from this head
  - **user** - Messages from humans, other heads, system, hands

### HandBundleBuilder (`hand_bundle.rs`)

Assembles the LLM conversation for a hand:

- System message: `hand_system.md` (identity) + `hand_grammar.md` (response format)
- Initial user message: task goal
- Conversation history: alternating assistant/user turns from tool calls

## Head Grammar

The head communicates via structured blocks:

```
--- chat #channel ---
message content
--- end ---

--- mail @recipient ---
message content
--- end ---

--- hand ---
list
goal "description of work"
read 0
clear 1
--- end ---
```

Hand commands:
- `list` - Show all hand slots with current state
- `goal "..."` - Create task, assign to next idle slot
- `read N` - Get details for hand N
- `clear N` - Reset hand N to idle (acknowledge completion)

## Hand Grammar

The hand responds with tool calls or results:

```
<exec tool="bash">
rg --files -g "*.rs" | wc -l
</exec>

<result ok="true">
Found 66 Rust files.
</result>

<result ok="false">
Could not find the requested file.
</result>
```

## Message Flow

1. Human sends chat: `"count the rust files"`
2. HeadService receives message, triggers `think()`
3. Head LLM responds with `goal "count rust files"` and acknowledgment chat
4. HeadService assigns goal to hand-0, publishes `TaskRequest` + `TaskAssigned`
5. HandService sees assignment, spawns execution loop
6. Hand LLM calls bash tool, gets result, emits `<result ok="true">`
7. HandService publishes `TaskResult`
8. HeadService receives result, updates slot to `success`, echoes to #general
9. HeadService triggers again, head clears slot and responds to human

## Logging

Set `RUST_LOG=info` to see the event flow:

| Event | Log Message |
|-------|-------------|
| Heartbeat | `ping tick=N` |
| Head thinking | `head thinking message_count=N` |
| Head LLM output | `--- HEAD RESPONSE ---` block |
| Slot assigned | `slot assigned head=X hand=N goal=...` |
| Task started | `task started task_id=... hand_id=... goal=...` |
| Hand LLM output | `--- HAND RESPONSE ---` block |
| Tool executed | `tool executed hand_id=... tool=... success=... duration_ms=...` |
| Task completed | `task completed task_id=... hand_id=... ok=...` |
| Slot completed | `slot completed head=X hand=N ok=...` |
| Slot cleared | `slot cleared head=X hand=N` |

## Configuration

### Head Config (`head_config.rs`)

| Env Var | Purpose | Default |
|---------|---------|---------|
| `HEAD_MODEL` | Model name (enables head) | - |
| `HEAD_API_KEY` | API key | - |
| `HEAD_BASE_URL` | API endpoint | OpenAI |
| `HEAD_TEMPERATURE` | Sampling temperature | 0.7 |
| `HEAD_MAX_TOKENS` | Max response tokens | - |
| `HEAD_HEARTBEAT_TICK` | Think every N ticks (0=disabled) | 10 |

### Hand Config (`hand_config.rs`)

| Env Var | Purpose | Default |
|---------|---------|---------|
| `HAND_MODEL` | Model name (enables hands) | - |
| `HAND_API_KEY` | API key | - |
| `HAND_BASE_URL` | API endpoint | OpenAI |
| `HAND_TEMPERATURE` | Sampling temperature | 0.2 |
| `HAND_MAX_TOKENS` | Max response tokens | - |
| `HAND_MAX_ITERS` | Max tool iterations | 24 |

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

## Failure Handling

**Hand failures:**
- Tool errors don't immediately fail the task
- Hand can retry up to 5 consecutive tool failures
- No action (no exec/result) for 3 iterations fails the task
- Iteration limit (default 24) fails the task
- All failures suggest "HEAD MUST PROVIDE: a clearer goal or break the task up."

**Slot contention:**
- If head issues `goal` when no slots are idle, the goal is dropped
- Head sees `[hand status] goal dropped (no slots available): ...`
- Head should use `list` to check availability and sequence work accordingly

## Scopes

Messages are published to scopes:

- `#channel` - Chat channels (e.g., `#general`)
- `@mailbox` - Direct messages
- `task/ID` - Task-specific scope (internal)

Task messages stay in their `task/ID` scope. Results are echoed to watched channels by the head.

## Files

```
runtime/
├── mod.rs              # exports
├── bus.rs              # RuntimeBus wrapper
├── head_service.rs     # head: orchestration, slot management
├── head_bundle.rs      # head conversation builder
├── head_config.rs      # head env config
├── head_parser.rs      # head response parser
├── head_grammar.md     # head response format spec
├── head_system.md      # head identity/conduct
├── hand_service.rs     # hand: tool execution loop
├── hand_bundle.rs      # hand conversation builder
├── hand_config.rs      # hand env config
├── hand_parser.rs      # hand response parser
├── hand_grammar.md     # hand response format spec
├── hand_system.md      # hand identity
└── exec.rs             # direct tool execution (non-LLM)
```
