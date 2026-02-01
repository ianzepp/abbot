# Abbot

A persistent AI background daemon built in Rust. Abbot runs continuously, keeps state in SQLite, and coordinates internal agents to respond to user input and execute tool-driven work.

Abbot exposes an OpenAI-compatible HTTP API (`/v1/...`) so external clients can talk to it like a provider, plus a web UI for direct interaction.

## Architecture: Mind / Head / Hand

Distributed-cognition model with recursive AI and pooled workers:

```
    Mind (strategic)         Head (tactical)           Hand (operational)
    ────────────────         ──────────────            ─────────────────
    proactive ticks          purely reactive           on-demand loops
    creates needs            converts needs→goals      executes tools
    maintains LTM            chats with users          returns results
    "why to do it"           "what to do"              "how to do it"

         │                         │                         │
         ▼                         ▼                         ▼
    ┌─────────┐              ┌───────────┐            ┌───────────┐
    │Conclave │──needs──────▶│NeedService│───────────▶│GoalService│
    │Mind/Head│              │(priority Q)│            │ (FIFO RR) │
    │  /Hand  │              │ head pool  │            │ hand pool │
    └─────────┘              └───────────┘            └───────────┘
```

**Flow:** Mind creates Need → NeedService → Head creates Goal → GoalService → Hand

- **Mind (Conclave)**: MindManager/HeadManager/HandManager deliberate to reach consensus on strategic needs, wants, and LTM updates
- **NeedService**: Priority queue dispatching needs to available heads (pool of 3)
- **Head**: Purely reactive - receives needs, converts to goals, responds to users, manages STM
- **GoalService**: FIFO queue with round-robin by scope, dispatching goals to hands (pool of 4)
- **Hand**: Executes goals using tools (`list_files`, `read_file`, `write_file`, etc.)

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

- `MIND_TICK` (default 60) - conclave deliberation interval
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

Head can also delegate to Hand via goal tools:

| Tool | Purpose |
|------|---------|
| `create_task` | Create goal with natural language |
| `search_files_goal` | Structured search delegation |

### Hand Tools (Operational Layer)

Hand executes file operations within the sandbox:

| Tool | Purpose |
|------|---------|
| `list_files` | List files (recursive, patterns) |
| `search_files` | Search file contents (regex) |
| `read_file` | Read file (with offset/limit) |
| `write_file` | Create/overwrite files |
| `apply_patch` | Apply unified diffs |
| `diff_files` | Compare two files |
| `mkdir` | Create directories |
| `git` | Run git commands |
| `curl` | Make HTTP requests |
| `add_want` | Add item to wants pool |

All file operations are validated against the sandbox workspace. Paths outside the workspace are rejected.

## How It Works (High Level)

**Startup (Boot Sequence):**

On the first tick after startup, Mind always wakes to orient itself:

- **Cold start** (no prior history): Mind receives `init.md` instructions to explore the workspace, look for `AGENTS.md` and `README.md`, identify the project type, and record findings to long-term memory.

- **Warm start** (prior history exists): Mind receives `boot.md` instructions plus system state (wants pool, recent needs/goals, stats) to check for incomplete work, review stale tasks, and resume operations.

Place an `AGENTS.md` file in your sandbox to provide Abbot with project-specific instructions, constraints, or context.

**User message flow:**
1. User message arrives (via HTTP `/v1/chat/completions`)
2. Message becomes a Need (normal priority) → NeedService queue
3. NeedService dispatches to available head from pool
4. Head processes need, creates goals if work needed, responds to user
5. GoalService assigns goals to hands via round-robin; hands execute and return results
6. Head receives goal results, may create follow-up goals or respond to user

**Mind proactive flow (Conclave):**
1. Mind wakes on heartbeat tick interval
2. Conclave convenes: MindManager, HeadManager, HandManager receive context (recent activity, LTM, wants pool)
3. Each mind proposes needs/wants/LTM updates and votes on others' proposals
4. Iterate until consensus (all agree) or max rounds (5)
5. Proposals with 2/3 votes become needs (immediate), wants (aspirational), or LTM updates (persistent)
6. Needs enter priority queue → dispatched to heads

## Project Structure

```
src/
├── bin/abbot.rs        # CLI entry point + daemon harness
├── runtime/            # Mind/Head/Hand services + NeedService/GoalService
│   ├── mind_*.rs       # Mind service, bundle, config
│   ├── head_*.rs       # Head service, bundle, config
│   ├── hand_*.rs       # Hand service, bundle, config
│   ├── need_service.rs # Priority queue dispatcher for needs
│   ├── goal_service.rs # FIFO queue dispatcher for goals
│   ├── room.rs         # Room/Conclave deliberation structure
│   └── conclave.rs     # Conclave deliberation loop
├── agent_tools.rs      # Tool definitions and execution
├── server/             # HTTP server
│   ├── mod.rs          # Server setup, static files, routing
│   ├── web_api.rs      # Web UI REST endpoints (/api/...)
│   └── websocket.rs    # Real-time updates via WebSocket
├── bus/                # Pub/sub messaging
├── history/            # SQLite storage (messages, memory, wants)
├── llm/                # Provider client (OpenAI-compatible)
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
