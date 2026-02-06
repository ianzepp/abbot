# Abbot CLI RPC (Draft)

This document specifies the local, trusted RPC transport used by `abbot-cli`
to interact with the long-running `abbot` daemon for scripting and automation.

## Binary Split

Abbot uses two binaries with a clear separation of concerns:

- **`abbot`** — Setup, configuration, and daemon lifecycle. Works on a cold system
  (no running daemon required). Includes: `run`, `service`, `reset`, `info` (static),
  `providers`, `plugin`, `tui`, `memory wipe`.
- **`abbot-cli`** — Runtime inspection, queries, and interaction. Requires a running
  daemon. Communicates exclusively over `rpc.sock`.

Rule of thumb: if the command reads runtime state or mutates something the daemon
owns, it belongs in `abbot-cli`. If it is filesystem setup, service management, or
config editing that should work when the daemon is off, it stays in `abbot`.

## Goals

- Reuse Abbot's existing internal protocol shape (kernel `Frame`) instead of inventing a new one.
- Provide a stable, script-friendly interface for management operations.
- Treat the local machine boundary as the trust boundary.

Non-goals:

- Hide protocol details from clients (a trusted local admin client can always introspect).
- Provide a public/remote auth model (that can be layered on later via HTTPS).

## Transport

Two separate Unix domain sockets, each with a single responsibility:

- **`<workspace>/rpc.sock`** — Bidirectional request/response for `abbot-cli`.
  Clients send requests and receive only their own correlated responses.
- **`<workspace>/frames.sock`** — One-way broadcast stream for TUI and monitoring
  clients. Sends all kernel frames to all connected clients (existing behavior).

Encoding (both sockets): NDJSON (one JSON object per line), where each line is a
serialized kernel `Frame`.

Rationale:

- UDS access is gated by filesystem permissions.
- NDJSON is easy to stream, parse, and pipe in shell scripts.
- Separate sockets avoid forcing CLI clients to filter broadcast traffic. A CLI
  command that sends one request and reads back a handful of responses should not
  have to discard hundreds of unrelated broadcast frames.
- Clients that need both capabilities (e.g., a future rich CLI monitor) can connect
  to both sockets.

## Protocol: Syscalls Over RPC

The RPC socket is a transport for Abbot's syscall protocol.

- Clients send a syscall request frame.
- The daemon dispatches it through the kernel dispatcher.
- The daemon streams back response frames (`ok`, `item`, `done`, `error`).

### Request

Use a normal request frame, and call the single RPC entrypoint syscall:

- `op`: `"req"`
- `name`: `"rpc:call"`
- `id`: client-generated UUID (used for correlation)
- `data`: JSON object

`rpc:call` payload:

```json
{
  "method": "audit.replay",
  "params": {"limit": 50},
  "scope": "main",
  "stream": true
}
```

Notes:

- `method` is a logical method name (string). The daemon maps methods to internal behavior.
- `params` is method-specific JSON.
- `scope` is optional. If omitted, the daemon defaults to `main`.
- `stream` is optional. If `true` (default), results may arrive as multiple `item` frames.

### Responses

Responses are standard kernel frames correlated by `parent_id`.

- `ok`: acknowledgement / metadata (optional)
- `item`: streamed result items
- `done`: end-of-stream
- `error`: error details

All response frames MUST set:

- `parent_id` to the request `id`.

Example (streaming list):

```json
{"id":"...","op":"ok","parent_id":"...","data":{"version":"0.1.0"}}
{"id":"...","op":"item","parent_id":"...","data":{"tool":{"name":"bash","calls":12}}}
{"id":"...","op":"item","parent_id":"...","data":{"tool":{"name":"read","calls":3}}}
{"id":"...","op":"done","parent_id":"...","data":{"count":2}}
```

Example (error):

```json
{"id":"...","op":"error","parent_id":"...","data":{"code":"bad_request","message":"missing params.limit"}}
{"id":"...","op":"done","parent_id":"..."}
```

### Cancellation

A client MAY cancel an in-flight request by sending a `cancel` frame:

- `op`: `"cancel"`
- `parent_id`: the original request `id`

The daemon SHOULD propagate cancellation through the kernel's `CancellationToken`
mechanism. If a client disconnects without sending `cancel`, the daemon SHOULD
treat the closed connection as an implicit cancellation of all outstanding requests
for that connection.

## Execution Context

### Trust model

- `rpc.sock` is intended for a trusted local admin client.
- The daemon SHOULD create the socket with `0600` permissions by default.

### Actor

The daemon forces `actor="user"` for all `rpc:call` dispatches.

Rationale:

- Aligns with Abbot's existing authorization semantics.
- Avoids accidental privilege escalation through user-controlled `actor` strings.

Note: `actor="user"` means RPC clients cannot invoke mutating syscalls that require
`head/*` actors. This is intentional for a management CLI. Methods that need to
trigger daemon-side mutations (e.g., `chat.send`) should be implemented as dedicated
RPC method handlers that perform the mutation internally with appropriate authority.

### CWD / workspace

The daemon dispatches `rpc:call` with a deterministic cwd of `<workspace>/root`.

Rationale:

- Matches the daemon's VFS root expectations.
- Makes filesystem-related syscalls consistent across clients.

## Versioning

- The daemon MUST emit an initial `ok` frame on connection, including protocol version
  (e.g., `{"version":"0.1.0"}`). This serves as a handshake confirmation.
- `rpc:call` methods SHOULD be versioned by method name stability and documented behavior.

If method contracts must change incompatibly:

- Prefer adding a new method name (e.g. `transcript.export_v2`) over changing semantics.

## Method Catalog

Methods that `abbot-cli` exposes over RPC. The CLI is a thin layer: parse clap args,
build the `rpc:call` payload, send one NDJSON line, read responses, format output.
All business logic lives in the daemon.

### Audit / Frames

- `audit.replay` — Replay recent frames, with optional kind/name filter and limit.
  Replaces current `abbot frames replay` (which queries `logs.db` directly).
- `audit.get` — Fetch a single frame by UUID.
  Replaces current `abbot frames get`.

### Chat

- `chat.send` — Send a user chat message to a given `scope`.

### Config

- `config.get` — Read config values from the running daemon.
- `config.set` — Update config values on the running daemon.

### Runtime Inspection

- `need.list` — List queued/active needs.
- `task.list` — List queued/active tasks.
- `room.list` — List active rooms.
- `conclave.recent` — List recent conclaves.
- `status.info` — Runtime status: uptime, pool sizes, queue depths, active agents.
  Complements the static `abbot info` command with live daemon state.

### Memory

- `memory.search` — Semantic search over recall index (requires embeddings).
  Replaces current `abbot memory search`.
- `memory.stats` — Recall index statistics.
  Replaces current `abbot memory stats`.
- `memory.index` — Trigger indexing of a directory. Long-running; streams progress
  frames. Replaces current `abbot memory index`.

### Transcript

- `transcript.export` — Export a time range into a portable markdown transcript.

## Commands Staying in `abbot`

These commands do not use RPC and remain in the main `abbot` binary:

| Command | Reason |
|---------|--------|
| `run` | Is the daemon |
| `service *` | Manages launchd/systemd; no daemon needed |
| `reset` | Wipes workspace files; should work when daemon is down |
| `info` (static) | Reads config files, checks paths/DB sizes; works offline |
| `providers *` | Setup tooling: reads/writes local cache files and `keys.env` |
| `plugin *` | Config tooling: reads/writes `abbot.toml` directly |
| `tui` | Launches a separate binary |
| `memory wipe` | Destructive; arguably requires daemon to be stopped |

## Implementation Notes (Server)

- The RPC socket server accepts multiple concurrent clients.
- Each inbound line is parsed as a `Frame`.
- Only `op=req` + `name=rpc:call` and `op=cancel` are accepted on this transport.
- For each request, the server translates `method` into internal dispatch (direct
  syscall invocation or dedicated handler) and streams back all response frames.
- The server MUST only send frames with matching `parent_id` to the requesting
  client — no broadcast traffic leaks onto `rpc.sock`.
- On parse errors, send a single `error` frame (with no `parent_id` if request id
  is unknown), then continue.
- On client disconnect, cancel all outstanding requests for that connection.

## Implementation Notes (Client)

- Connect to `<workspace>/rpc.sock`.
- Read the initial `ok` handshake frame to confirm protocol version.
- Generate a UUID for each request (`id`).
- Write one JSON-serialized request frame + `\n`.
- Read response frames until `done` with matching `parent_id`.
- For scripts, treat each `item` as a record; pipe through `jq` as needed.
- On timeout or interrupt, send a `cancel` frame before disconnecting.
