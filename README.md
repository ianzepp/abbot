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
- `fs:*`, `git:*`, `net:*`, `proc:*`: constrained host operations

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

## Tooling Model (Internal vs Plugin vs External)

Internal tools are defined as OpenAI function tools and executed in-process:

- Head tools (`head__*`): planning, bounded reads, controlled mutation, enqueueing tasks
- Hand tools (`hand__*`): operational tooling; hands are enforced read-only by policy
- Mind tools (`mind__*`): strategic/conclave/autonomy operations

Plugins are compiled-in command tools loaded from `daemon/src/plugins/*/plugin.toml` and enabled via `[plugins]` in `abbot.toml`. They can be exposed to head and/or hand, with separate expose/exec policy and output limits. Hands may not execute write-level plugins.

External tools are registered at runtime by clients and are executed out-of-process by the client (via redirects).

## Prompt Bundling and Memory

Abbot constructs role prompts by layering a small set of fixed "system slots" (identity, commandments, context, tools, environment, memory, tone). Each role fills a different subset:

- Head: identity + commandments + head tools + hand tools (delegation) + external tool summaries + behavior + environment + LTM + tone, plus optional cached user system prompt
- Hand: commandments + hand tools + environment + tone, with the task goal/input as the primary user message (and head STM injected)
- Mind: commandments + wake prompt (init/boot/normal) + mind tools + (optional) environment + tone, with Self/LTM + recent activity summarized into the user message

Memory is represented as:

- Self: durable collective identity (`mind/self.md`)
- LTM: durable long-term memory (`mind/memory.md`)
- STM: short-term working memory owned by heads and injected into hand tasks

## Storage and Directory Layout

All abbot state lives under `~/.abbot/`:

- `abbot.toml`: main config
- `keys.env`: API keys (loaded as env vars)
- `providers/*.json`: cached provider model lists
- `config.toml`: runtime overrides (agent-writable)
- `mind/memory.md`: long-term memory (LTM)
- `mind/self.md`: collective identity (Self)
- `store.db`: conversation + tool registry + misc state
- `ems.db`: entity store
- `frames.db`: frame audit log
- `daemon.log`: written when launching with a TUI frontend

The agent's working directory and VFS root is `~` (the user's home directory).

## Configuration

Global config: `~/.abbot/abbot.toml` (schema: `daemon/src/runtime/app_config.rs`).

VFS mounts are configured under `[vfs].mounts` as `{ prefix, host, mode }`.

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
- `abbotd plugin detect|list|set <id> <none|read|write>`: manage plugin access levels
- `abbotd memory index|stats|search|wipe`: semantic memory management
- `abbotd tui`: spawn `abbot-tui`
- `abbotd frames get|replay`: query `logs.db` kernel frame audit
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
- `daemon/src/runtime/`: mind/head/hand services, prompt bundling, snapshots, plugins
- `daemon/src/server/`: OpenAI/Anthropic APIs, web chat, websocket, admin endpoints
- `tui/`: terminal UI (monitor + chat + explorer + config + logs)
- `web/`: Leptos/Trunk frontend served from `web/dist/`

## License

Non-commercial use only. All rights reserved.
