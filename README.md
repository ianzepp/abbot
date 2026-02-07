# Abbot

Abbot is a persistent, tool-using AI daemon built in Rust.

It runs as a long-lived process, stores state in SQLite, and exposes an OpenAI-compatible API so other clients can talk to it like a provider. Internally it uses a message-first microkernel with syscall-style isolation.

## Installation

### Homebrew (macOS/Linux)

```bash
brew install ianzepp/tap/abbot
```

### Manual Download

Download binaries from [Releases](https://github.com/ianzepp/abbot-releases/releases).

### From Source

```bash
cargo install --git https://github.com/ianzepp/abbot.git
```

## Quick Start

```bash
# Configure a provider
abbotd providers login anthropic

# Switch to a model
abbotd providers use anthropic/claude-3-5-haiku-latest

# Check configuration
abbotd info

# Run the daemon
abbotd run
```

## How Abbot Is Structured

Abbot is organized as three cooperating roles, coordinated by a kernel:

- Mind (strategic): wakes on SIGTICK, convenes autonomy/conclave rooms
- Head (tactical): reacts to user needs, plans, delegates to hands, writes replies
- Hand (operational): executes tool-driven work loops (no direct user chat)

The kernel is the only place services meet.

## Kernel: Frames, Streaming, Backpressure

All communication is a stream of Frames routed through `KernelDispatcher`.

Frame fields (wire format is JSON):

- `id`: unique frame id; for `Req` this becomes the syscall call_id
- `parent_id`: correlation; syscall responses use the request `id`; reply-stream frames typically use a thread id
- `op`: `req|cancel|ok|error|done|redirect|item|bytes|event|progress`
- `name`: syscall name for `Req`; optional semantic tag for other ops (e.g. `chat:message`, `tool:request`)
- `actor`: authorization identity (e.g. `head/<id>`, `hand/<id>`, `system/...`)
- `data`: syscall payloads, streamed items, bytes chunks, tool redirects, etc.

Backpressure is enforced per stream: if a consumer stops draining, the kernel pauses producers and can cancel the call after a stall timeout.

## Syscalls (Conceptual)

Syscalls are namespaced operations registered into the kernel (see `daemon/src/syscalls/`). Common namespaces:

- `need:*`: enqueue/lease/fulfill work for heads
- `task:*`: enqueue/lease/complete tasks for hands
- `room:*`: deliberation rooms (autonomy/conclave)
- `tick:*`: SIGTICK subscription
- `log:*`: audit/event append and frame queries
- `tool:*`: external tool registry and tool result delivery
- `fs:*`, `git:*`, `net:*`, `exec:*`: constrained host operations

Lane routing matters: long-polling syscalls like `need:lease` and `task:lease` are forced onto the immediate lane to avoid deadlocks with enqueue/complete.

## Scopes, Reply Streams, and External Tools

Scopes are the conversation/tenant boundary.

- Local interactive usage defaults to `scope = "main"`.
- OpenCode-style clients are assigned a stable `session/<hash>` scope derived from the Authorization token + the client-reported working directory.

Replies are delivered via a per-(scope, thread_id) reply stream managed by `SigcallHub`. The HTTP layer opens a reply stream first, then enqueues work (so a head can immediately write bytes into the stream).

External tools (client-provided tools):

- Clients can send OpenAI-style `tools` in `POST /v1/chat/completions`.
- Abbot registers those tool schemas under the current scope (`tool:register`).
- Heads see them as tool calls named `user__<toolname>`.
- When a head calls `user__...`, Abbot does not execute it. It emits a terminal `Redirect` frame into the reply stream and closes the transport stream.
- The client executes the tool and submits results back by sending a follow-up request with trailing `role:"tool"` messages; Abbot routes those to `tool:result` and resumes the head's in-progress need.

## Tooling Model (Internal vs External)

Internal tools are defined as OpenAI function tools and executed in-process:

- Head tools (`head__*`): planning, bounded reads, controlled mutation, enqueueing tasks
- Hand tools (`hand__*`): operational tooling; hands are enforced read-only by policy
- Mind tools (`mind__*`): strategic/conclave/autonomy operations

External tools are registered at runtime by clients and are executed out-of-process by the client (via redirects).

## Prompt Bundling and Memory

Abbot constructs role prompts by layering a small set of fixed "system slots" (identity, commandments, context, tools, environment, memory, tone). Each role fills a different subset:

- Head: identity + commandments + head tools + hand tools (delegation) + external tool summaries + behavior + environment + memories + tone, plus optional cached user system prompt
- Hand: commandments + hand tools + environment + tone, with the task goal/input as the primary user message (and head STM injected)
- Mind: commandments + wake prompt (init/boot/normal) + mind tools + (optional) environment + tone, with memories + recent activity summarized into the user message

Memory is stored as EMS entities (`kind = "memory"` in the unified entities table). Heads and minds query memories from EMS at bundle time. Short-term working memory (STM) is owned by heads and injected into hand tasks.

## Storage and Directory Layout

All abbot state lives under `~/.abbot/`:

- `abbot.toml`: main config
- `keys.env`: API keys (loaded as env vars)
- `providers/*.json`: cached provider model lists
- `config.toml`: runtime overrides (agent-writable)
- `store.db`: conversation + tool registry + misc state
- `ems.db`: entity store
- `frames.db`: frame audit log
- `daemon.log`: written when launching with a TUI frontend

The agent's working directory and VFS root is `~` (the user's home directory).

## Configuration

Global config lives at `~/.abbot/abbot.toml`. Generated by `abbot init`.
Workspace overrides in `~/.abbot/config.toml` (agent-writable, same field names).
Priority: workspace > abbot.toml > hardcoded default.

### Top-level

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `developer` | bool | `false` | Enable developer/dogfood mode (agents report problems and suggest improvements to Abbot itself) |

### `[server]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `addr` | string | `"127.0.0.1:8080"` | API server bind address (host:port) |
| `log_format` | string | `"default"` | Log output format: `default`, `compact`, `pretty` |
| `proxy_base_url` | string | — | Upstream base URL for transparent proxy mode |
| `web_dist` | string | — | Path to `web/dist` directory for the built-in UI |
| `allow_loopback_main_scope` | bool | `false` | Allow localhost peers to use main scope without session markers |
| `allow_cors_any` | bool | `false` | Allow permissive CORS (`*`) for cross-origin development clients |

### `[providers.<name>]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `base_url` | string | — | Provider API base URL |
| `api_key_env` | string | — | Environment variable name holding the API key |

### `[llm]`

Shared LLM defaults inherited by head, hand, and mind.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `model` | string | — | Model ID in `provider/model` format |
| `temperature` | float | `0.7` | Sampling temperature |
| `max_tokens` | u32 | — | Maximum output tokens |

### `[traits]`

Global trait selections (personality/behavioral directives). Each key is a trait category; value is the selected variant name or `"none"` to disable.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `fever` | string | `"none"` | Fever trait variant |
| `generation` | string | `"none"` | Generation trait variant |
| `autist` | string | `"none"` | Autist trait variant |
| `filter` | string | `"none"` | Filter trait variant |
| `poverty` | string | `"none"` | Poverty trait variant |
| `ego` | string | `"none"` | Ego trait variant |
| `paranoia` | string | `"none"` | Paranoia trait variant |
| `cultist` | string | `"none"` | Cultist trait variant |
| `dominance` | string | `"none"` | Dominance trait variant |
| `bipolar` | string | `"none"` | Bipolar trait variant |
| `xenophobe` | string | `"none"` | Xenophobe trait variant |
| `esoteric` | string | `"none"` | Esoteric trait variant |
| `collab` | string | `"none"` | Collab trait variant |

### `[head]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `traits` | string[] | `[]` | Override global traits for heads |
| `heartbeat_tick` | u64 | `30` | Heartbeat interval in seconds |
| `debounce_ms` | u64 | `500` | Debounce delay in milliseconds |
| `time_gap_marker_minutes` | u64 | — | Insert time-gap marker after this many minutes of silence (0 to disable) |
| `pool` | usize | `3` | Number of head instances in the pool |

### `[hand]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `traits` | string[] | `[]` | Override global traits for hands |
| `max_iters` | usize | `24` | Maximum tool-use iterations per task |
| `max_output_chars_in_prompt` | usize | `12000` | Max tool output characters included in prompt |
| `max_trace_entries_in_prompt` | usize | `6` | Max trace entries included in prompt |
| `pool` | usize | `4` | Number of hand instances in the pool |

### `[mind]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `traits` | string[] | `[]` | Override global traits for mind |
| `tick_interval` | u64 | `60` | Seconds between mind wake cycles |

### `[prompt_cache]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | — | Enable user system prompt compaction/caching |
| `model` | string | — | Model for prompt compaction (inherits from `[llm]`) |
| `temperature` | float | — | Temperature for compaction |
| `max_tokens` | u32 | — | Max tokens for compaction |

### `[harness]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `model` | string | — | Fallback model when `[llm].model` is not set |
| `slow_idle` | u64 | `5` | Minutes until slow_idle fires |
| `deep_idle` | u64 | `60` | Minutes until deep_idle fires |

### `[vfs]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `mounts` | array | `[]` | VFS mount points as `{ prefix, host, mode }` objects |

## Running

Build:

```bash
cargo build
```

Run the daemon (first run auto-creates `~/.abbot/abbot.toml` with defaults):

```bash
cargo run -p abbot-daemon -- run
```

Launch the TUI (daemon must already be running):

```bash
cargo run -p abbot-tui -- --addr 127.0.0.1:8080
```

## CLI Commands

The main binary is `abbotd` (see `daemon/src/bin/abbot.rs`). Primary commands:

- `abbotd run [opencode|claude|web|prompt <text>]`: run daemon, optionally launch a frontend
- `abbotd reset [--force] [--config]`: wipe workspace databases and state
- `abbotd providers refresh|list|add|remove|test|use`: manage provider keys + cached model lists
- `abbotd tui`: spawn `abbot-tui`
- `abbotd frames get|replay`: query `frames.db` kernel frame audit
- `abbotd monitor [--filter <pattern>]`: live frame stream from WebSocket

## HTTP + WebSocket API

OpenAI-compatible:

- `GET /v1/models`
- `POST /v1/chat/completions` (supports streaming; redirects external tools as tool_calls)

Anthropic-compatible:

- `POST /v1/messages` (streaming supported; tool calls are not)

Web UI chat:

- `POST /api/chat` (SSE; not OpenAI wire format)

WebSocket:

- `GET /ws`: streams `{type:"frame"}` messages for kernel frames and supports client ping/pong

Admin (localhost-only):

- `GET/PUT /admin/config` and `/admin/config/{section}`
- `GET /admin/providers/models`
- `GET /admin/fs/list`, `GET /admin/fs/read`
- `GET /admin/logs`

Proxy mode:

- `abbotd --proxy run` forwards OpenAI-compatible requests to `server.proxy_base_url` and disables the rest of the server features.

## Web UI Build Notes

The `web/` directory is a Leptos (Rust/WASM) frontend built with Trunk (`Trunk.toml`) and emitted to `web/dist/`. The Rust server can serve that dist directory when configured.

Note: `web/package.json` and `web/README.md` currently contain a Vite/React scaffold that does not match the current Rust/Trunk frontend sources.

## Repo Map

- `daemon/src/kernel/`: frame protocol, dispatcher/router, audit log, sigcall hub, need/task/room kernels
- `daemon/src/syscalls/`: syscall implementations registered into the kernel
- `daemon/src/runtime/`: mind/head/hand services, prompt bundling, snapshots
- `daemon/src/server/`: OpenAI/Anthropic APIs, web chat, websocket, admin endpoints
- `tui/`: terminal UI (monitor + chat + explorer + config + logs)
- `web/`: Leptos/Trunk frontend served from `web/dist/`

## License

Non-commercial use only. All rights reserved.
