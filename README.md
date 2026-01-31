# Abbot

A persistent AI background daemon built in Rust. Abbot runs continuously, keeps state in SQLite, and coordinates internal agents to respond to user input and execute tool-driven work.

Abbot also exposes an OpenAI-compatible HTTP API (`/v1/...`) so external clients (like OpenCode) can talk to it like a provider.

## Architecture: Mind / Head / Hand

Distributed-cognition model with recursive AI:

```
    Mind (strategic)         Head (tactical)           Hand (operational)
    ────────────────         ──────────────            ─────────────────
    proactive ticks          purely reactive           on-demand loops
    creates needs            converts needs→goals      executes tools
    maintains LTM            chats with users          returns results
    "why to do it"           "what to do"              "how to do it"
```

**Flow:** Mind creates Need → NeedService → Head creates Goal → GoalService → Hand

- **Mind**: wakes on heartbeat, reviews activity/memory, creates strategic needs
- **Head**: receives needs (from Mind or users), converts to goals, responds to users
- **Hand**: executes goals using tools (`bash`, `read`, `write`, etc.)

**Services:**
- **NeedService**: priority queue dispatching needs to head pool
- **GoalService**: FIFO queue dispatching goals to hand pool

## Quick Start

1) Build

```bash
cargo build
```

2) Configure

- `config.toml` selects models (by ID) and runtime knobs.
- `models.toml` maps model IDs to provider base URLs and API-key env var names.
- `.env` is for secrets (API keys). Do not commit it.

Example `.env`:

```bash
OPENAI_API_KEY=sk-...
```

3) Run

```bash
./target/debug/abbot run
```

4) Talk to it

- OpenAI-compatible API: `http://127.0.0.1:8080/v1`
- Default model served by the API: `abbot/default`

## Configuration

Abbot loads configuration from:

1. `config.toml` (default path configurable via `ABBOT_CONFIG`)
2. `models.toml` (in repo root)
3. Environment variables / `.env` (for overrides and secrets)

### Key env vars

- `ABBOT_DB`: path to SQLite message DB (default `abbot.db`)
- `ABBOT_MEMORY_DB`: path to memory/vector DB (default `memory.db`)
- `ABBOT_CONFIG`: config file path (default `config.toml`)
- `ABBOT_ADDR`: HTTP server addr (default `127.0.0.1:8080`)

### LLM selection and overrides

By default, `config.toml` points at a model ID in `models.toml` (format: `provider/model`).

You can override per service via env vars:

- `HEAD_MODEL`, `HEAD_BASE_URL`, `HEAD_API_KEY`, `HEAD_TEMPERATURE`, `HEAD_MAX_TOKENS`
- `HAND_MODEL`, `HAND_BASE_URL`, `HAND_API_KEY`, `HAND_TEMPERATURE`, `HAND_MAX_TOKENS`
- `MIND_MODEL`, `MIND_BASE_URL`, `MIND_API_KEY`, `MIND_TEMPERATURE`, `MIND_MAX_TOKENS`

Runtime knobs:

- `HEAD_HEARTBEAT_TICK` (default 60)
- `HEAD_DEBOUNCE_MS` (default 500)
- `HAND_MAX_ITERS` (default 24)
- `MIND_TICK` (default 60)

## CLI

```bash
# Run daemon (default)
abbot run

# Run daemon and inject an initial prompt
abbot --prompt "Hello" 

# Exit once the head finishes processing the prompt chain
abbot --prompt "Hello" --exit

# Memory index management
abbot memory index path/to/transcripts
abbot memory stats
abbot memory search what did we decide about X
abbot memory wipe

# OpenCode integration
abbot opencode register
abbot opencode run
```

## Tools Available to Hands

Hands execute tools via the runtime tool dispatcher (`src/tools/*`). Current tools:

| Tool | Purpose |
|------|---------|
| `bash` | Run shell commands |
| `read` | Read file contents |
| `write` | Create/overwrite files |
| `edit` | Modify files (search/replace) |
| `find` | Find files by pattern |
| `diff` | Compare files or git state |
| `patch` | Apply unified diffs |
| `cd` | Change working directory |
| `recall` | Search indexed transcripts (enabled when memory DB is available) |

## How It Works (High Level)

**User message flow:**
1. User message arrives (via HTTP `/v1/chat/completions`)
2. Message becomes a Need (normal priority) → NeedService queue
3. NeedService dispatches to available head
4. Head processes need, creates goals if needed, responds to user
5. GoalService assigns goals to hands; hands execute and return results

**Mind proactive flow:**
1. Mind wakes on heartbeat tick
2. Reviews recent activity, LTM, strategic context
3. Creates needs based on patterns, commitments, opportunities
4. Needs enter priority queue → dispatched to heads

## Project Structure

```
src/
├── bin/abbot.rs        # CLI entry point + daemon harness
├── runtime/            # Head/Mind/Hands runtime services
├── server/             # OpenAI-compatible HTTP API (/v1/...)
├── bus/                # Pub/sub messaging
├── history/            # SQLite storage
├── llm/                # Provider client (OpenAI-compatible)
└── tools/              # Tool implementations + dispatcher
```

## License

MIT
