# Abbot

A persistent AI background daemon built in Rust. Abbot runs continuously, keeps state in SQLite, and coordinates internal agents to respond to user input and execute tool-driven work.

Abbot exposes an OpenAI-compatible HTTP API (`/v1/...`) so external clients can talk to it like a provider, plus a web UI for direct interaction.

## Architecture: Kernel-Driven Mind / Head / Hand

Message-first microkernel with distributed cognition:

```
    Mind (strategic)         Head (tactical)           Hand (operational)
    ────────────────         ──────────────            ─────────────────
    proactive ticks          purely reactive           on-demand loops
    creates needs            converts needs→goals      executes tools
    maintains LTM            chats with users          returns results
    "why to do it"           "what to do"              "how to do it"

         │                         │                         │
         └─────────────┬───────────┴───────────┬─────────────┘
                       ▼                       ▼
                 ┌──────────────────────────────────┐
                 │         Kernel Dispatcher        │
                 │  (syscalls, streaming, backpressure) │
                 └──────────────────────────────────┘
                       │                       │
                  need:enqueue            task:enqueue
                  need:lease              task:lease
                  room:create             tick:subscribe
                  log:append              ...
```

**Flow:** Mind creates Need → `need:enqueue` syscall → Head leases via `need:lease` → Head creates Goal → `task:enqueue` syscall → Hand leases via `task:lease`

- **Kernel**: Frame-based protocol (`Req`/`Ok`/`Error`/`Done`/`Item`/`Progress`), streaming with backpressure, syscall dispatch, SIGTICK broadcasting
- **Mind (Conclave)**: MindManager/HeadManager/HandManager deliberate to reach consensus on strategic needs, wants, and LTM updates
- **Head**: Purely reactive - leases needs, converts to tasks, responds to users, manages STM
- **Hand**: Leases tasks, executes tools via HAL (Hardware Abstraction Layer)

**Core layers:**
- **Kernel** (`src/kernel/`): Syscall dispatcher, Frame protocol, backpressure, tick broadcasting
- **EMS** (`src/ems/`): Entity Management System - schema-flexible SQLite entity store
- **VFS** (`src/vfs/`): Virtual File System - mount-based filesystem isolation
- **HAL** (`src/hal/`): Hardware Abstraction Layer - fs, git, net, process interfaces

## Memory Architecture

```
Conclave ──► Self (collective identity)
         ──► LTM (long-term memory)
               │
               ▼ (injected into context)
            Heads ──► STM (short-term memory)
               │
               ▼ (injected into context)
            Hands
```

- **Self (Collective Identity)**: Who we are as a system — values, principles, character. Managed by the Conclave via proposals (append/replace/remove); requires 2/3 consensus. Self flows into all contexts.

- **LTM (Long-Term Memory)**: Strategic, persistent learnings managed by the Conclave. Minds propose LTM updates (append/replace/remove) during deliberation; requires 2/3 consensus. LTM flows automatically into heads.

- **STM (Short-Term Memory)**: Tactical, working context managed by heads via `read_stm`/`update_stm` tools. STM flows automatically into hands when tasks are created.

## Kernel & Frame Protocol

All service communication flows through the kernel dispatcher via **Frames**:

### Frame Types

| Op | Direction | Purpose |
|----|-----------|---------|
| `Req` | → kernel | Request a syscall (e.g., `need:enqueue`, `task:lease`) |
| `Ok` | ← kernel | Single-value response (terminal) |
| `Done` | ← kernel | Stream termination (no more items) |
| `Error` | ← kernel | Error response (terminal) |
| `Redirect` | ← kernel | Redirect to external tool (sigcall) |
| `Item` | ← kernel | Stream item (non-terminal) |
| `Bytes` | ← kernel | Binary stream chunk |
| `Event` | ← kernel | Event notification |
| `Progress` | ← kernel | Progress update |
| `Cancel` | → kernel | Cancel running request |

### Backpressure

Kernel maintains per-stream watermarks. When producer fills the buffer:
- Producer pauses at high-water mark
- Consumer drains frames
- Producer resumes at low-water mark

Consumer can `Cancel` any time. Kernel detects stalled consumers (no drain activity for timeout) and auto-aborts.

### Syscall Namespaces

| Namespace | Purpose | Examples |
|-----------|---------|----------|
| `need:*` | Need lifecycle | `enqueue`, `lease`, `ack`, `fulfill` |
| `task:*` | Task lifecycle | `enqueue`, `lease`, `progress`, `result`, `cancel` |
| `room:*` | Deliberation rooms | `create`, `join`, `propose`, `vote`, `close` |
| `tick:*` | Timer signals | `subscribe`, `unsubscribe` |
| `log:*` | Audit logging | `append`, `select` |
| `reply:*` | Reply streams | `send`, `close` |

External tools (via Opencode CLI) are routed through the kernel as `Redirect` frames (sigcall pattern).

## EMS & VFS

### EMS (Entity Management System)

Schema-flexible SQLite entity store:
- **Schema-on-write**: Tables/columns created lazily when data arrives
- **All TEXT columns**: JSON encoding for nested structures, avoids type mismatches
- **Operations**: `insert`, `update`, `delete`, `select`, `query`
- Used for: wants pool, conversation history, task state, memory snapshots

### VFS (Virtual File System)

Mount-based filesystem isolation:
- **No access without mounts**: Empty mount table = filesystem disabled
- **Longest-prefix matching**: Most specific mount wins, supports nesting
- **Symlink escape detection**: Warns when symlinks resolve outside mounts
- **Read-only enforcement**: Mounts can be marked `ro` to prevent writes

All Hand file operations go through VFS → HAL. Paths outside configured mounts are rejected.

## Quick Start

1) Build

```bash
cargo build
```

2) Configure

Create config files in `~/.config/abbot/`:

- `abbot.toml` - selects models (by ID) and runtime knobs
- `models.toml` - maps model IDs to provider base URLs and API-key env var names

API keys must be set in your shell environment (e.g., `export OPENAI_API_KEY=sk-...`).

Example `~/.config/abbot/abbot.toml`:

```toml
[head]
model = "openai/gpt-4.1"
temperature = 0.7

[hand]
model = "openai/gpt-4.1-mini"
temperature = 0.2
max_iters = 24

[mind]
model = "ollama/llama3.2"
tick_interval = 60

[pool]
size = 4
timeout_secs = 300
```

3) Run

```bash
./target/debug/abbot run
```

4) Talk to it

- **Web UI**: `http://127.0.0.1:8080/` (three-panel interface: file tree, chat, activity)
- **OpenAI-compatible API**: `http://127.0.0.1:8080/v1`
- Default model served by the API: `abbot/default`

## Sandboxing

Abbot runs in an isolated sandbox. All file operations are contained within the sandbox workspace.

### Sandbox Management

```bash
# Create a new sandbox
abbot sandbox create myproject

# Clone a git repo into a new sandbox
abbot sandbox clone https://github.com/user/repo.git
abbot sandbox clone https://github.com/user/repo.git --name custom-name

# List all sandboxes
abbot sandbox list

# Show detailed sandbox status
abbot sandbox status
abbot sandbox status myproject

# Reset a sandbox (wipe data, keep mounts)
abbot sandbox reset myproject

# Delete a sandbox and all its data
abbot sandbox delete myproject
```

### Sandbox Paths

| Path | Purpose |
|------|---------|
| `~/.local/abbot/<sandbox>/root/` | Workspace (all file ops contained here) |
| `~/.local/abbot/<sandbox>/store.sqlite` | Persistent database (messages, memory, wants) |
| `~/.local/abbot/<sandbox>/recall.sqlite` | Vector database (semantic search) |

### Mounting External Directories

To work on real projects, mount external directories into the sandbox:

```bash
# Add a mount (creates symlink)
abbot mount add myproject ~/code/myproject

# List mounts
abbot mount list

# Remove a mount
abbot mount remove myproject
```

Mounts are symlinks inside the sandbox. Path validation is lexical, so Abbot can follow symlinks for I/O but cannot escape the sandbox namespace.

### Running with a Specific Sandbox

```bash
# Run with default sandbox
abbot run

# Run with specific sandbox
abbot --sandbox myproject run
```

### Fever Mode

Fever mode controls how proactive and creative the Mind layer is. Higher fever = more initiative, less caution.

```bash
abbot run --fever mild      # Be more exploratory
abbot run --fever hot       # Take initiative, less hedging
abbot run --fever delirium  # Fuck it, we ball
abbot run --fever meth      # The walls are breathing
```

| Mode | Behavior |
|------|----------|
| (none) | Caretaker mode. Waits for input. |
| mild | Considers more possibilities, leans toward action |
| hot | Generates needs aggressively, less hedging |
| delirium | Proactively builds, searches, experiments |
| meth | Immediate autonomy on any idle. Never reaches conclave. Always doing, never reflecting. |

Fever prompts are defined in `src/fever/*.md` and injected into the Mind's system prompt.

## Configuration

Abbot loads configuration from `~/.config/abbot/`:

| File | Purpose |
|------|---------|
| `abbot.toml` | Main config (models, runtime settings) |
| `models.toml` | Model definitions (providers, URLs, API keys) |

### Key env vars

- `ABBOT_SANDBOX`: sandbox name (default `default`)
- `ABBOT_CONFIG`: config file path (default `~/.config/abbot/abbot.toml`)
- `ABBOT_ADDR`: HTTP server addr (default `127.0.0.1:8080`)

### LLM selection and overrides

By default, `abbot.toml` points at a model ID in `models.toml` (format: `provider/model`).

You can override per service via env vars:

- `HEAD_MODEL`, `HEAD_BASE_URL`, `HEAD_API_KEY`, `HEAD_TEMPERATURE`, `HEAD_MAX_TOKENS`
- `HAND_MODEL`, `HAND_BASE_URL`, `HAND_API_KEY`, `HAND_TEMPERATURE`, `HAND_MAX_TOKENS`
- `MIND_MODEL`, `MIND_BASE_URL`, `MIND_API_KEY`, `MIND_TEMPERATURE`, `MIND_MAX_TOKENS`

Runtime knobs:

- `MIND_TICK` (default 60) - mind tick interval (autonomy/conclave triggered by idle events)
- `HEAD_DEBOUNCE_MS` (default 500) - debounce before head thinks
- `HAND_MAX_ITERS` (default 24) - max tool iterations per goal

### Logging

Set `RUST_LOG` to control verbosity:

```bash
RUST_LOG=info abbot run    # Flow + decisions (default)
RUST_LOG=debug abbot run   # Include internal details
```

At `info` level you'll see:
- Startup and shutdown
- Needs dispatched/fulfilled
- Goals dispatched/completed
- Head tool calls and responses
- Mind proposals

At `debug` level you'll also see:
- Service configuration
- Message routing
- Conclave rounds
- Queue operations

## CLI

```bash
# Run daemon (default sandbox)
abbot run

# Run daemon with specific sandbox
abbot --sandbox myproject run

# Run daemon and inject an initial prompt
abbot --prompt "Hello" 

# Exit once the head finishes processing the prompt chain
abbot --prompt "Hello" --exit

# Sandbox management
abbot sandbox create <name>
abbot sandbox clone <git-url> [--name <name>]
abbot sandbox list
abbot sandbox status [<name>]
abbot sandbox reset <name>
abbot sandbox delete <name>

# Mount management
abbot mount add <name> <path>
abbot mount remove <name>
abbot mount list

# Memory index management
abbot memory index path/to/transcripts
abbot memory stats
abbot memory search what did we decide about X
abbot memory wipe

# Export history as transcript
abbot export                          # export current sandbox to stdout
abbot export myproject                # export named sandbox
abbot export --output history.txt     # write to file

# OpenCode integration
abbot opencode register
abbot opencode run
```

## Tools

### Head Tools (Tactical Layer)

Head has direct access to bounded read-only tools:

| Tool | Purpose | Constraints |
|------|---------|-------------|
| `read_file` | Read file section | Requires `offset` and `limit`, max 100 lines |
| `list_files` | List directory contents | Requires `max_results`, max 50 |
| `recall` | Search semantic memory | - |
| `introspect` | Query system state | - |
| `send_message` | Send chat message | - |
| `read_stm` | Read short-term memory | - |
| `update_stm` | Update short-term memory | ops: set, append, clear |
| `convene_conclave` | Request mind deliberation | - |

Head delegates to Hand via syscalls:

| Syscall | Purpose |
|---------|---------|
| `task:enqueue` | Create task with natural language instruction |
| `task:cancel` | Cancel running task |

### Hand Tools (Operational Layer via HAL)

Hand executes operations via Hardware Abstraction Layer (HAL) within VFS mounts:

| Tool | Purpose | HAL Interface |
|------|---------|---------------|
| `list_files` | List files (recursive, patterns) | `HalFs` |
| `search_files` | Search file contents (regex) | `HalFs` |
| `read_file` | Read file (with offset/limit) | `HalFs` |
| `write_file` | Create/overwrite files | `HalFs` |
| `apply_patch` | Apply unified diffs | `HalFs` |
| `diff_files` | Compare two files | `HalFs` |
| `mkdir` | Create directories | `HalFs` |
| `git` | Run git commands | `HalGit` |
| `curl` | Make HTTP requests | `HalNet` |
| `add_want` | Add item to wants pool | EMS |

All file operations go through VFS mount resolution. Paths outside configured mounts are rejected.

## How It Works (High Level)

**Startup (Boot Sequence):**

On the first tick after startup, Mind always wakes to orient itself:

- **Cold start** (no prior history): Mind receives `init.md` instructions to explore the workspace, look for `AGENTS.md` and `README.md`, identify the project type, and record findings to long-term memory.

- **Warm start** (prior history exists): Mind receives `boot.md` instructions plus system state (wants pool, recent needs/tasks, stats) to check for incomplete work, review stale tasks, and resume operations.

Place an `AGENTS.md` file in your sandbox to provide Abbot with project-specific instructions, constraints, or context.

**User message flow (kernel-driven):**
1. User message arrives (via HTTP `/v1/chat/completions`)
2. Server calls `need:enqueue` syscall with message payload
3. Head service polls via `need:lease` syscall, acquires need
4. Head processes need, creates tasks if work needed, responds to user via `reply:send`
5. Hand services poll via `task:lease` syscall, execute tools via HAL, return results via Frame protocol (`Item`/`Ok`/`Done`)
6. Head receives task results (streamed frames), may create follow-up tasks or respond to user

**Mind proactive flow (SIGTICK → Autonomy & Conclave):**

Kernel broadcasts SIGTICK on a timer. Mind subscribes via `tick:subscribe` and triggers meetings on idle:

| Meeting | Trigger | Purpose |
|---------|---------|---------|
| Autonomy | 5 min idle | Operational retro: what happened, what's next? |
| Conclave | 1 hour idle | Strategic: who are we, how should we grow? |

Meeting flow:
1. Mind calls `room:create` syscall to create deliberation room
2. MindManager, HeadManager, HandManager receive context (via `log:select` to query recent activity, LTM, wants pool)
3. Each mind proposes and votes on others' proposals (via `room:*` syscalls)
4. Iterate until consensus (all agree) or max rounds (5)
5. Proposals with 2/3 votes are executed (via `need:enqueue`, `ltm:update`, etc.)

Autonomy focuses on needs (what to do next) and wants (deferred work).
Conclave focuses on Self (identity), LTM (memory), and strategic wants.

## Project Structure

```
src/
├── bin/abbot.rs        # CLI entry point + daemon harness
├── kernel/             # Kernel dispatcher + syscall infrastructure
│   ├── dispatcher.rs   # Frame routing, backpressure, streaming
│   ├── frame.rs        # Frame protocol (Req/Ok/Error/Done/Item/...)
│   ├── syscall.rs      # Syscall trait + context
│   ├── needs.rs        # need:enqueue/lease/ack/fulfill syscalls
│   ├── tasks.rs        # task:enqueue/lease/progress/result syscalls
│   ├── rooms.rs        # room:create/join/propose/vote syscalls
│   ├── tick.rs         # SIGTICK broadcasting + tick:subscribe
│   ├── external_tools.rs # External tool routing (sigcall-like)
│   ├── log_select.rs   # log:select syscall (query conversation history)
│   └── audit.rs        # Frame audit logging to logs.db
├── runtime/            # Mind/Head/Hand services (kernel-driven)
│   ├── mind_*.rs       # Mind service, bundle, config
│   ├── head_*.rs       # Head service, bundle, config
│   ├── hand_*.rs       # Hand service, bundle, config
│   ├── kernel.rs       # Runtime kernel harness
│   ├── room.rs         # Room structure for deliberation
│   └── conclave.rs     # Autonomy/Conclave deliberation loop
├── ems/                # Entity Management System (schema-flexible SQLite)
│   ├── service.rs      # EmsService handle + operations
│   └── tools.rs        # EMS tool exposure to agents
├── vfs/                # Virtual File System (mount-based isolation)
│   ├── mount.rs        # Mount table, path resolution
│   └── config.rs       # Mount configuration
├── hal/                # Hardware Abstraction Layer
│   ├── fs.rs           # Filesystem operations (via VFS)
│   ├── git.rs          # Git command execution
│   ├── net.rs          # HTTP requests (curl)
│   └── process.rs      # Process spawning
├── fever/              # Fever mode prompts (mild, hot, delirium, meth)
├── server/             # HTTP server
│   ├── handler.rs      # OpenAI-compatible /v1/chat/completions
│   ├── web_api.rs      # Web UI REST endpoints (/api/...)
│   ├── websocket.rs    # Real-time updates via WebSocket
│   └── anthropic.rs    # Anthropic API compatibility layer
├── llm/                # Provider client (OpenAI + Anthropic)
└── memory/             # Semantic memory (embeddings + search)

web/                    # React frontend (Vite + TypeScript)
├── src/
│   ├── components/     # FileTree, ChatPanel, ActivityPanel, etc.
│   ├── store/          # Zustand state management
│   └── api/            # REST client
└── dist/               # Built assets (served by Rust backend)
```

## License

MIT
