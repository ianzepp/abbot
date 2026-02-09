```
 ▗▄███▄▖
  █◉ ◉█
  ⠿ ⠿ ⠿
```

# Abbot

Abbot is a persistent, tool-using AI daemon built in Rust.

It runs as a long-lived process, stores state in SQLite, and exposes OpenAI-compatible APIs so other clients can talk to it like a provider. Internally it’s a message-first microkernel: everything is a `Frame`, and services interact only by sending syscall-style `Req` frames through the kernel dispatcher and consuming streamed responses.

This repo ships multiple binaries:

- `abbotd`: the daemon (kernel + agent runtime + HTTP/WebSocket ingress)
- `abbot`: the CLI (setup, config/providers, lifecycle, and RPC commands)
- `abbot-tui`: multi-room chat TUI (WebSocket streaming + replay)
- `abbot-monitor`: monitoring dashboard (UDS frame stream + config/logs UI)

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
# First-time setup (writes ~/.abbot/abbot.toml and ~/.abbot/keys.env)
abbot init

# Configure a provider + pick a model
abbot providers login anthropic
abbot use anthropic claude-sonnet-4-20250514

# Start the daemon (service-managed if available)
abbot start

# Send a message to the default room ("main")
abbot chat send "hello"

# Optional: live frame stream (WebSocket)
abbot monitor --filter "chat:*"
```

Foreground daemon (useful while developing):

```bash
cargo run -p abbot-daemon -- run
```

## CLI / TUI / Monitor

- `abbot` (CLI): setup + lifecycle + diagnostics + RPC.
  - Setup: `abbot init`, `abbot providers …`, `abbot use …`, `abbot config …`, `abbot mounts …`
  - Lifecycle: `abbot start|stop|restart|status`, `abbot service …`
  - Interaction: `abbot chat send …`, `abbot tui`, `abbot dashboard`, `abbot run opencode|claude|tui`
  - Observability: `abbot monitor`, `abbot frames …`, `abbot doctor`, `abbot info`
- `abbot-tui`: chat client with multiple room tabs, streaming responses, and reconnect replay.
- `abbot-monitor`: dashboard for frames/needs/tasks/tools/replies, plus config editor and log viewer.

## Mental Model

- Everything is a `Frame`.
- The kernel is the only place services meet.
- The “agent” layer is implemented as roles (head/hand/mind) running inside rooms and interacting via syscalls and tool dispatch.

## Kernel: Frames, Streaming, Backpressure

All internal communication is a stream of Frames routed through `KernelDispatcher`.

Common Frame fields (wire format is JSON):

- `id`: unique frame id; for `Req` this becomes the syscall call_id
- `parent_id`: correlation; syscall responses use the request `id`; reply-stream frames typically use a thread id
- `op`: `req|cancel|ok|error|done|redirect|item|bytes|event|progress`
- `name`: syscall name for `Req`; optional semantic tag for other ops (e.g. `chat:message`, `tool:request`)
- `actor`: authorization identity (e.g. `head/<id>`, `hand/<id>`, `system/...`)
- `data`: syscall payloads, streamed items, bytes chunks, tool redirects, etc.

Backpressure is enforced per stream: if a consumer stops draining, the kernel pauses producers and can cancel the call after a stall timeout.

## Syscalls (Conceptual)

Syscalls are namespaced operations registered into the kernel (see `daemon/src/syscalls/`). Common namespaces:

- `chat:*`: interactive message injection and streaming
- `need:*`: queue, lease, and fulfill “needs” (units of work)
- `room:*`: run/list/reschedule rooms (parallel agent execution contexts)
- `tick:*`: SIGTICK subscription (mind loop cadence)
- `tool:*`: external tool registry and tool result delivery
- `frames:*`: query the frame audit database
- `fs:*`, `git:*`, `net:*`, `exec:*`: constrained host operations (policy gated)

Lane routing matters: long-polling syscalls like `need:lease` must not share the same lane lock as enqueue/complete.

## Rooms, Doors, and Interactive Chat

Abbot tags frames with a `room` string for isolation and replay. A room is both an isolation label and an execution context:

- **Room**: a named context (`"main"`, `"<session-hash>"`, `"<custom-name>"`) that runs one or more agents in parallel rounds
- Room names are plain strings — no prefixes or type conventions

Interactive chat flows through `chat:message`:

- User messages are injected into a room (creating the room on first use).
- Agent responses stream back through a **Door** (typically `WebSocketDoor`) into the reply stream.
- Room execution continues asynchronously; clients observe frames via `/ws`, `frames.sock`, or `frames.db`.

## Tools (Internal vs External)

Abbot uses two tool models:

- Internal tools: executed in-process, policy-gated by actor identity and syscall/tool dispatch rules.
  - Room/hand tool specs live under `daemon/src/runtime/room/*.json` (e.g. `hand__edit`, `hand__shell`, `hand__test`).
- External tools: registered by clients at runtime via OpenAI-compatible `tools`.
  - Registered per room via `tool:register`, exposed to heads as `user__<toolname>`.
  - Calling `user__…` emits a terminal `redirect`; clients execute the tool and submit results back as `role:"tool"` messages (handled by `tool:result`).

## Resilience: Safe Mode Provider Failover

When the active LLM provider is unhealthy (timeouts/5xx/auth/transport failures), Abbot enters **safe mode**: it pauses LLM work, scans cached model lists, and selects the first healthy provider+model it can probe. Cached model lists are stored at `~/.abbot/providers/<provider>.json`.

## Storage and Directory Layout

All Abbot state lives under `~/.abbot/`:

- `abbot.toml`: main config (generated by `abbot init`)
- `keys.env`: API keys (loaded into environment on daemon start)
- `providers/*.json`: cached provider model lists
- `store.db`: conversation + tool registry + misc state
- `ems.db`: entity store (memories + entities)
- `frames.db`: frame audit log (queryable via syscalls / CLI)
- `rpc.sock`: daemon RPC socket for `abbot` CLI (0600 permissions)
- `frames.sock` (unix): daemon frame stream socket for `abbot-monitor`
- `mind/`: mind loop scratch and transcripts (implementation-defined)
- `sandbox/`: VFS sandbox root

The agent’s VFS root is always `~` (the user’s home). Abbot maintains a sandbox-backed VFS root at `~/.abbot/sandbox/` and mounts additional host paths explicitly via `[vfs.mounts]`.

## Configuration

Global config lives at `~/.abbot/abbot.toml` and is generated by `abbot init`.

Practical entrypoints:

- `abbot config show` / `abbot config set …`
- `abbot providers …` (login/list/refresh/use/test)
- `abbot mounts …` (VFS mount points)

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
- `GET /admin/rooms` (list rooms observed in frames)

Proxy mode:

- `abbotd --proxy run` forwards OpenAI-compatible requests to `server.proxy_base_url` and disables the rest of the server features.

## Web UI Build Notes

The `web/` directory is a Leptos (Rust/WASM) frontend built with Trunk (`Trunk.toml`) and emitted to `web/dist/`. The Rust server can serve that dist directory when configured.

Note: `web/package.json` and `web/README.md` currently contain a Vite/React scaffold that does not match the current Rust/Trunk frontend sources.

## Repo Map

- `daemon/`: the daemon crate (`abbotd`) and shared library (`abbot`)
- `cli/`: the CLI crate (`abbot`)
- `tui/`: the chat TUI crate (`abbot-tui`)
- `monitor/`: the dashboard crate (`abbot-monitor`)
- `macos/`: macOS `.app` wrapper + packaging scripts
- `web/`: Leptos/Trunk frontend (excluded from the Rust workspace)
- `docs/`: design notes and refactor plans (not always current)

## License

Non-commercial use only. All rights reserved.
