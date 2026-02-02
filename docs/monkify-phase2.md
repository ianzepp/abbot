# Monkify Phase 2 (Concrete Design)

This document expands `abbot/docs/monkify-overview.md` into a concrete architecture plan.

Assumption: if Abbot's current implementation conflicts with this target design, the current behavior is incorrect and should be refactored to match the design.

The agreed starting implementation focus is:

- (2) Stream controller + protocol backpressure
- (4) Sigcall kernelization (external tool bridge)

## 1) Kernel protocol: streams are the contract

Abbot should treat every syscall/tool invocation as:

`Request -> Stream<Response>`

Where responses are structured frames with explicit lifecycle semantics.

### Canonical stream ops

Non-terminal:

- `Item` (structured records)
- `Bytes` (binary/text chunks)
- `Event` (typed event + payload)
- `Progress` (structured progress updates)

Terminal:

- `Ok` (final successful result)
- `Error` (final error)
- `Done` (explicit end-of-stream when there is no `Ok`/`Error` payload)
- `Redirect` (handoff instruction; primarily used for sigcall/external-tool execution)

Design rule: terminal ops end the stream deterministically; consumers must not infer termination from channel close.

### Control plane

Control frames are used to safely bridge streams across boundaries and enforce backpressure:

- `Ping { processed }` (consumer ack)
- `Cancel` (consumer abort)

These controls are protocol-level (not just in-process mechanics).

## 2) Backpressure model: make it protocol-level

Abbot currently has internal backpressure via bounded queues. Monk's model makes backpressure explicit and portable.

Target behavior:

- Producers can emit arbitrarily many stream items.
- The kernel inserts a controller that:
  - tracks `sent` vs `acked` (via `Ping(processed)`)
  - pauses production at `HIGH_WATER`
  - resumes at `LOW_WATER`
  - aborts stalled streams if no pings for `STALL_TIMEOUT`

Two consumption modes:

1) Ping-capable consumers (internal services, Opencode sigcall bridge)
2) Ping-incapable consumers (SSE/websocket streaming): rely on bounded channels and awaiting writes

## 3) Process model: services act like userspace processes

Even if implemented as tokio tasks, model runtime services as processes:

- `ProcessContext` holds identity, scope/session, cwd, permissions, active streams.
- Processes interact via syscalls/ports, not direct shared-state mutation.

## 4) Sigcall as a kernel facility (external tools)

Abbot should implement Monk-style sigcall, but in Abbot terms:

- External tools (provided by Opencode) are *userspace handlers*.
- Internal tools/syscalls remain kernel handlers.

### Registry

- Registry is per `session_scope`.
- Maps `tool_name -> handler` (in practice, the connected Opencode session).
- Updated whenever the client provides a tool list.

### Routing

Introduce a unified entry point for tool execution:

- `tool:<name>(args) -> stream<Response>`

Routing rules:

- If `tool:<name>` is internal: execute locally.
- Else if registered as external in `session_scope`: route via sigcall.
- Else return `Error NOT_IMPLEMENTED`.

### Wire mapping (OpenAI tool_calls)

For external tools, Abbot emits a `Redirect` terminal frame that the HTTP layer converts into an OpenAI `tool_call`:

- kernel -> client: `Redirect { tool_call_id, name, arguments_json }`
- client -> kernel: `sigcall:response { tool_call_id, output }` (as an OpenAI tool result)

The kernel owns correlation and stream resumption state, not the head service.

## 5) Replace most of the bus with point-to-point syscalls

Keep pub/sub only where broadcast is the correct primitive:

- UI activity feed
- audit log / persistence stream
- coarse system events

Most internal coordination becomes explicit syscalls or "ports" (stream endpoints).

## 6) SIGTICK: kernel-owned clock signal

- Implement `tick:subscribe` / `tick:unsubscribe`.
- Deliver `{ dt, now, seq }`.
- Use ticks for autonomy/heartbeat scheduling.

## 7) Intelligent proxy (Provider <-> Abbot <-> Opencode)

Abbot acts like a microkernel between:

- upstream model provider (OpenAI-compatible)
- Opencode CLI (external tool runner + UX)

Abbot rewrites prompts and tool exposure and routes tool calls to either:

- internal kernel handlers
- external sigcall handlers (Opencode)

## Implementation focus (agreed)

Start with:

- (2) Stream controller + protocol backpressure
- (4) Sigcall kernelization

These two changes make Abbot a real "intelligent proxy" quickly, without requiring the full bus removal on day one.
