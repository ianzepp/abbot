# Abbot

A persistent AI background agent framework built in Rust. Abbot runs continuously, watches channels, responds to humans, and delegates work to tool-using workers.

## Architecture: Heart / Head / Hands

Abbot uses an octopus-inspired model where cognition is distributed:

```
    Heart (soul)              Head (will)              Hands (means)
    ────────────              ──────────               ────────────
    large model               large model              small model
    slow ticks (60s)          fast ticks (10s)         on-demand
    reflects                  decides                  executes
    edits LTM                 delegates goals          runs tools
    no output                 chats, mails             returns results
    "what interests me"       "what to do now"         "how to do it"
```

**Heart** - The soul. Periodically reflects on what the head has been doing, notices patterns and interests, updates long-term memory (LTM). Never speaks directly.

**Head** - The will. Watches channels, responds to humans, delegates work to hands via goals. Reads LTM to inform decisions.

**Hands** - The means. Execute goals using tools (bash, read, write, edit, find, etc.). Return results to head.

## Quick Start

```bash
# Build
cargo build

# Configure (.env)
cat > .env << 'EOF'
HEAD_MODEL=gpt-4.1
HEAD_API_KEY=sk-...
HAND_MODEL=gpt-4.1-mini
HAND_API_KEY=sk-...
HEART_MODEL=gpt-4.1
HEART_API_KEY=sk-...
HEART_TICK=60
EOF

# Run server
./target/debug/abbot server run

# In another terminal, chat
./target/debug/abbot chat "#general" "Hello!"
./target/debug/abbot tail "#general" --limit 20
```

## Configuration

All configuration via environment variables (or `.env` file):

### Head (orchestrator)
| Variable | Description | Default |
|----------|-------------|---------|
| `HEAD_MODEL` | LLM model name | (required) |
| `HEAD_API_KEY` | API key | (required) |
| `HEAD_BASE_URL` | API endpoint | OpenAI |
| `HEAD_TEMPERATURE` | Sampling temp | 0.7 |
| `HEAD_HEARTBEAT_TICK` | Think every N ticks | 10 |

### Hand (worker)
| Variable | Description | Default |
|----------|-------------|---------|
| `HAND_MODEL` | LLM model name | (required) |
| `HAND_API_KEY` | API key | (required) |
| `HAND_BASE_URL` | API endpoint | OpenAI |
| `HAND_TEMPERATURE` | Sampling temp | 0.2 |
| `HAND_MAX_ITERS` | Max tool iterations | 24 |

### Heart (reflection)
| Variable | Description | Default |
|----------|-------------|---------|
| `HEART_MODEL` | LLM model name | (optional) |
| `HEART_API_KEY` | API key | (optional) |
| `HEART_BASE_URL` | API endpoint | OpenAI |
| `HEART_TICK` | Reflect every N ticks | 60 |

## CLI Commands

```bash
# Server
abbot server run              # Start server (foreground)
abbot server run --heartbeat-s 1  # Fast ticks for dev
abbot server status           # Check if running
abbot server stop             # Stop server

# Chat
abbot chat "#general" "message"   # Send message to channel
abbot tail "#general"             # Watch channel
abbot tail "#general" --limit 50  # Last 50 messages
```

## Tools Available to Hands

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

## How It Works

1. Human sends message to `#general`
2. Head sees message, decides to respond
3. Head may delegate work: `goal "count rust files"`
4. Hand-0 picks up goal, runs tools, returns result
5. Head sees result, responds to human
6. Heart (periodically) reflects on activity, updates LTM
7. Head's future decisions are influenced by LTM

## Logging

Set `RUST_LOG=info` to see event flow:

```
ping tick=10
head thinking message_count=3
--- HEAD RESPONSE ---
...
slot assigned hand=0 goal="count files"
task started task_id=abc123
--- HAND RESPONSE ---
...
tool executed tool=bash success=true duration_ms=34
task completed ok=true
heart reflecting tick=60
--- HEART RESPONSE ---
...
ltm append content="Curious about: Rust patterns"
ltm saved ltm_len=45
```

## Project Structure

```
src/
├── bin/abbot.rs        # CLI entry point
├── runtime/            # Heart/Head/Hands implementation
│   ├── heart_*.rs      # Heart service, config, parser, bundle
│   ├── head_*.rs       # Head service, config, parser, bundle
│   ├── hand_*.rs       # Hand service, config, parser, bundle
│   ├── bus.rs          # Message bus wrapper
│   └── README.md       # Detailed runtime docs
├── bus/                # Pub/sub messaging
├── history/            # SQLite storage
├── api/                # HTTP API server
├── irc/                # IRC server (optional)
├── llm/                # LLM client
└── tools/              # Tool implementations
```

## Future: The Monastery Layer

The `apps/monastery/` directory and files like `VISION.md` describe a higher-level vision: multiple monks (agents) coordinating as a community, with commandments, hierarchy, and shared rituals.

The heart/head/hands model is the foundation. The monastery layer would add:
- Multiple monks with different personalities
- Shared values and rules (commandments)
- Coordination between monks
- Promotion/dismissal based on performance

This remains a future direction. The current implementation focuses on a single head with its heart and hands.

Note: `apps/monastery/` contains a first-pass implementation of the monastery concept with its own LLM wiring, context model, and tools (garden, pray, etc.). It's kept for reference but is not actively maintained. The core `src/runtime/` implementation supersedes it.

## License

MIT
