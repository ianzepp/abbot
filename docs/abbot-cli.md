# Abbot CLI RPC (Draft)

This document specifies the local, trusted RPC transport used by a future `abbot-cli`
to interact with the long-running `abbot` daemon for scripting and automation.

## Goals

- Reuse Abbot's existing internal protocol shape (kernel `Frame`) instead of inventing a new one.
- Provide a stable, script-friendly interface for management operations.
- Treat the local machine boundary as the trust boundary.

Non-goals:

- Hide protocol details from clients (a trusted local admin client can always introspect).
- Provide a public/remote auth model (that can be layered on later via HTTPS).

## Transport

- Unix domain socket: `<workspace>/rpc.sock`
- Encoding: NDJSON (one JSON object per line)
- Each line is a serialized kernel `Frame`.

Rationale:

- UDS access is gated by filesystem permissions.
- NDJSON is easy to stream, parse, and pipe in shell scripts.

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
  "method": "tools.recent",
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

## Execution Context

### Trust model

- `rpc.sock` is intended for a trusted local admin client.
- The daemon SHOULD create the socket with `0600` permissions by default.

### Actor

Recommended default behavior:

- The daemon forces `actor="user"` for all `rpc:call` dispatches.

Rationale:

- Aligns with Abbot's existing authorization semantics.
- Avoids accidental privilege escalation through user-controlled `actor` strings.

### CWD / workspace

Recommended default behavior:

- The daemon dispatches `rpc:call` with a deterministic cwd of `<workspace>/root`.

Rationale:

- Matches the daemon's VFS root expectations.
- Makes filesystem-related syscalls consistent across clients.

## Versioning

- The daemon MAY emit an initial `ok` frame including protocol info.
- `rpc:call` methods SHOULD be versioned by method name stability and documented behavior.

If method contracts must change incompatibly:

- Prefer adding a new method name (e.g. `transcript.export_v2`) over changing semantics.

## Method Catalog (Initial Targets)

This is an initial wish list for CLI-driven management. Exact schemas are TBD.

- `audit.replay`
  - Replay last N frames or a time range (from `logs.db`).
- `chat.send`
  - Send a user chat message to a given `scope`.
- `config.get`, `config.set`, `config.replace`
  - Read/update config values.
- `need.list`, `task.list`, `room.list`
  - Inspect queued/running needs/tasks/rooms.
- `http.recent`
  - Show recent user HTTP requests to `/api/v1/*` (requires ingress logging).
- `conclave.recent`
  - List recent conclaves.
- `transcript.export_markdown`
  - Export a time range into a portable markdown transcript.

## Implementation Notes (Server)

- Socket server accepts multiple clients.
- Each inbound line is parsed as a `Frame`.
- Only `op=req` + `name=rpc:call` are accepted on this transport.
- For each request, the daemon dispatches a kernel request and streams back all response frames.
- On parse errors, send a single `error` frame (with no `parent_id` if request id is unknown), then continue.

## Implementation Notes (Client)

- Generate a UUID for each request (`id`).
- Write one JSON-serialized request frame + `\n`.
- Read response frames until `done` with matching `parent_id`.
- For scripts, treat each `item` as a record; pipe through `jq` as needed.
