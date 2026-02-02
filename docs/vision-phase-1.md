# Vision Phase 1: Kernel Message Protocol

Phase 1 introduces a message-first kernel protocol inside Abbot.

This phase is about unifying *effectful execution* under a single request/response stream abstraction that is:
- messages-in / messages-out (tagged unions)
- streaming by default
- cancelable

This does not require replacing Abbot's existing bus; it layers a syscall-style protocol on top of the bus (or a sibling transport), using a stable envelope.

## Concepts

- `name`: the semantic operation being invoked, e.g. `fs:read`, `need:create`.
- `op`: the protocol frame type, e.g. `req`, `ok`, `error`, `item`, `bytes`, `event`, `progress`.
- `id`: correlates a request with its response stream.
- `scope`: routes and partitions streams (e.g. `tasks/<id>`).

The critical separation is:
- `name` answers: what are we doing?
- `op` answers: what kind of frame is this?

## Transport Model

All kernel interactions are modeled as a stream of frames.

- One `req` starts a call.
- Zero or more `item`/`bytes`/`event`/`progress` frames stream back.
- One terminal frame ends the call: `ok` or `error`.

Some calls may return only `ok`.
Others may stream for a long time and return `ok` only at the end.

Frame ordering depends on the transport; the protocol provides no ordering guarantee beyond terminal frame semantics.

## Envelope

Each frame carries the same envelope.

Required envelope fields:
- `id`: UUID string for the call
- `op`: frame discriminator (see below)

Common envelope fields:
- `name`: present on `req` (and optionally echoed on responses)
- `parent_id`: optional UUID string to relate to a higher-level call
- `scope`: routing scope (`main` or `plural-type/uuid`, e.g. `tasks/<id>`, `heads/<id>`)
- `deadline_ms`: optional request deadline
- `trace`: optional tracing metadata (span ids, etc)

Payload field:
- `data`: op-specific payload (JSON object) or bytes for `bytes` frames

## Frame Ops

The protocol uses a fixed set of `op` values.

Request/control ops:
- `req`: start a call
- `cancel`: request cancellation of an in-flight call

Response ops:
- `ok`: terminal success
- `error`: terminal failure
- `item`: streamed item (typed per `name`, represented as JSON)
- `bytes`: streamed bytes chunk
- `event`: streamed structured event (typed per `name`)
- `progress`: streamed progress update

Notes:
- `ok` and `error` are the only terminal frames.
- `cancel` is a control-plane op and should be accepted at any time after `req`.

## Errors

`error` payload is structured.

Required fields:
- `code`: stable error code (e.g. `E_INVALID_ARGS`, `E_FORBIDDEN`, `E_TIMEOUT`, `E_CANCELLED`)
- `message`: human-readable message

Optional fields:
- `help`: human-readable suggestion for how to fix or proceed
- `detail`: JSON for machine-readable context
- `retryable`: boolean

### LLM-Oriented Error Guidance

Many kernel calls are consumed by an LLM. Errors should therefore be written to be actionable without additional context.

Guidelines:
- `message` states what failed (short, factual). `help` states what to do next (concrete, permissive, and within policy).
- Prefer one next-step over multiple choices. If multiple are necessary, keep it to 2-3.
- Include a minimal example of a correct call shape when the fix is syntactic or policy-related.
- Avoid meta-advice ("check logs"). If logs matter, include what to look for.
- Keep `help` safe: do not suggest reading secrets or broadening access; suggest the least-privilege alternative.

Examples:

1) Invalid args

```json
{
  "op": "error",
  "data": {
    "code": "E_INVALID_ARGS",
    "message": "fs:read: path is empty",
    "help": "Provide a non-empty VFS path under /workspace, e.g. { \"name\": \"fs:read\", \"op\": \"req\", \"data\": { \"path\": \"/workspace/README.md\", \"limit\": 2000 } }"
  }
}
```

2) Forbidden

```json
{
  "op": "error",
  "data": {
    "code": "E_FORBIDDEN",
    "message": "proc:run: program 'rm' is not allowed",
    "help": "Use an allowed program (e.g. git, cargo, npm) or perform a safe read-only inspection first. If you need deletion, request an explicit allowlist update for this operation."
  }
}
```

3) Cancelled

```json
{
  "op": "error",
  "data": {
    "code": "E_CANCELLED",
    "message": "git:run: cancelled",
    "help": "Stop emitting further frames for this call id. If you still need the result, re-issue the request with a new id and a longer deadline."
  }
}
```

## Cancellation

Cancellation is explicit and best-effort, but should be prompt.

`cancel` payload:
- `reason`: string

Semantics:
- The kernel should attempt to stop work and release resources.
- A cancelled call should end with `error` using `E_CANCELLED` (or `ok` if the work already completed).
- Cancel succeeds silently on already-completed calls.

## Namespacing

Operation names follow a `namespace:verb` format.

Examples:
- `fs:read`, `fs:write`
- `proc:run`, `proc:spawn`
- `net:fetch`
- `git:run`
- `need:create`

## Routing and Scopes

Kernel calls are scoped.

Scope format: `main` | `plural-type/uuid`

Recommendations:
- Calls originating from a task use `scope = tasks/<task_id>`.
- Calls originating from a head use `scope = heads/<head_id>`.
- System-level calls use `scope = main`.
- The scope is used for access control and for UI grouping.

## Suggested Phase 1 Syscall Surface

Phase 1 does not require a large syscall surface.
Start with a small set to validate the protocol and streaming behavior.

Minimum viable set:
- `fs:read`
- `fs:write`
- `proc:run` (bounded stdout/stderr; cancel kills child)
- `net:fetch` (bounded body; timeouts)
- `git:run`

Optional non-effectful (but useful) to test message routing:
- `need:create` (internal state mutation)
- `task:cancel` (bridges to existing TaskService cancellation)

## Mapping to Abbot Internals

This phase is intentionally compatible with Abbot's current architecture.

- The "kernel" is a dispatcher that receives `req` frames and emits response frames.
- Hand/Head code emits `req` frames rather than directly calling tool implementations.
- Existing tool functions remain, but get moved behind syscall handlers incrementally.

Recommended initial integration path:
1) Add kernel frame types (Rust enums) and serialization.
2) Implement dispatcher with a registry keyed by `name`.
3) Implement one syscall end-to-end (recommend `proc:run`).
4) Bridge one existing tool to call the syscall path.
5) Add `cancel` handling and verify bounded memory.

## Wire Format (JSON)

For Phase 1, use JSON for frames.

Example request:

```json
{ "id": "...", "op": "req", "name": "proc:run", "scope": "tasks/abc", "data": { "program": "git", "argv": ["status"], "cwd": "/workspace" } }
```

Example response stream:

```json
{ "id": "...", "op": "progress", "data": { "current": 1, "total": 3 } }
{ "id": "...", "op": "bytes", "data": "c3Rkb3V0IGNodW5r..." }
{ "id": "...", "op": "ok", "data": { "code": 0 } }
```

Binary chunks:
- `bytes` frames carry data as base64 in JSON for Phase 1.
- Later phases can upgrade transport (CBOR, WS binary frames) without changing the logical protocol.

## Observability

Every `req` should produce structured tracing:
- start/end timestamps
- `name`, `scope`, `id`
- output sizes (bytes/items)
- termination reason (`ok`/`error`, error code)

Phase 1 should at minimum log:
- `req` received
- `ok`/`error` emitted
- cancellation events
