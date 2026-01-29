# Runtime

The runtime implements a head/hand architecture for task execution, modeled on an octopus: the head is the will, the hands are the means.

## Architecture

```
                    ┌─────────────┐
                    │   Human     │
                    │  (CLI/IRC)  │
                    └──────┬──────┘
                           │ task request
                           ▼
┌──────────────────────────────────────────────────────┐
│                     RuntimeBus                        │
│  (pub/sub message passing + sqlite persistence)       │
└──────────────────────────────────────────────────────┘
        │                    │                    │
        ▼                    ▼                    ▼
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│ HeadService  │    │HandAllocator │    │ HandService  │
│              │    │              │    │              │
│ tracks tasks │    │ assigns hand │    │ executes via │
│ reports to   │    │ IDs to new   │    │ LLM + tools  │
│ #general     │    │ task requests│    │              │
└──────────────┘    └──────────────┘    └──────────────┘
```

## Components

### RuntimeBus (`bus.rs`)

Central message passing. Wraps the pub/sub Hub and sqlite Store. All components publish and subscribe through the bus.

### HeadService (`head_service.rs`)

Tracks task lifecycle. When a task completes (success or failure), reports the result to a designated channel (default: `#general`). The head is the coordinator - it knows what tasks are in flight and their outcomes.

### HandAllocator (`hand_allocator.rs`)

Assigns unique hand IDs to incoming task requests. When a `TaskMsg::Request` arrives, the allocator generates a `hand-XXXXXXXX` ID and publishes a `TaskMsg::Assigned` message. This decouples task creation from execution.

### HandService (`hand_service.rs`)

The worker. When a task is assigned, the hand service:

1. Builds a conversation using `HandBundleBuilder`
2. Calls the LLM
3. Parses the response for `--- exec ---` or `--- result ---` blocks
4. Executes tools via the `Dispatcher`
5. Logs results to sqlite
6. Repeats until `--- result ---` or iteration limit

The hand doesn't decide what to do - it executes the head's intent using available tools.

### HandBundleBuilder (`hand_bundle.rs`)

Assembles the LLM conversation:

- System message: `hand_system.md` (identity) + `hand_grammar.md` (response format)
- Initial user message: task goal and input
- Conversation history: alternating assistant/user turns from sqlite

Returns `Vec<ChatMessage>` ready for the LLM.

### HandConfig (`hand_config.rs`)

LLM configuration from environment:

- `HAND_MODEL` - model name (enables LLM mode)
- `HAND_API_KEY` - API key
- `HAND_BASE_URL` - API endpoint (default: OpenAI)
- `HAND_TEMPERATURE` - sampling temperature
- `HAND_MAX_TOKENS` - max response tokens
- `HAND_MAX_ITERS` - max tool iterations per task

### Hand Parser (`hand_parser.rs`)

Parses hand responses. Format:

```
--- exec TOOL [key=value ...] ---
content
--- end ---

--- result ok ---
summary
--- end ---

--- result fail ---
what went wrong
--- end ---
```

Returns `ParsedHandResponse` with `Vec<ExecAction>` and optional `ResultAction`.

## Message Flow

1. Human submits task via CLI: `abbot task new "find where Config is defined"`
2. API server publishes `TaskMsg::Request` to `§task/t-XXXXX`
3. HandAllocator sees request, publishes `TaskMsg::Assigned` with new hand ID
4. HeadService records the task metadata
5. HandService sees assignment, spawns execution:
   - Build conversation via HandBundleBuilder
   - Call LLM
   - Parse response
   - If `--- exec ---`: run tool, log to DB, loop
   - If `--- result ---`: publish `TaskMsg::Result`, done
6. HeadService sees result, publishes summary to `#general`

## Scopes

Messages are published to scopes:

- `#channel` - chat channels (e.g., `#general`)
- `@mailbox` - direct messages
- `§task/ID` - task-specific scope

Task messages stay in their `§task/ID` scope. Final results are reported to channels by the head.

## Tools

Available to the hand via `Dispatcher`:

| Tool | Purpose |
|------|---------|
| bash | Run shell commands |
| read | Read file contents |
| write | Create/overwrite files |
| edit | Modify files (OLD/NEW blocks) |
| find | Find files by pattern |
| diff | Compare files or git state |
| patch | Apply unified diffs |
| cd | Change working directory |

## Failure Handling

- Tool errors don't immediately fail the task
- Hand can retry up to 5 consecutive failures
- No action (no exec/result) for 3 iterations fails the task
- Iteration limit (default 24) fails the task

All failures report what went wrong and suggest "HEAD MUST PROVIDE: a clearer goal or break the task up."

## Files

```
runtime/
├── mod.rs              # exports
├── bus.rs              # RuntimeBus
├── head_service.rs     # task tracking, result reporting
├── head_bundle.rs      # head context builder (not yet used)
├── head_grammar.md     # head response format (not yet used)
├── head_system.md      # head identity (not yet used)
├── hand_allocator.rs   # assigns hand IDs
├── hand_service.rs     # LLM execution loop
├── hand_bundle.rs      # conversation builder
├── hand_config.rs      # env config
├── hand_parser.rs      # response parser
├── hand_grammar.md     # response format spec
├── hand_system.md      # hand identity
└── exec.rs             # direct tool execution service
```
