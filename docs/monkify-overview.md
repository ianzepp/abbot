# Monkify Overview (Monk OS Kernel -> Abbot)

This doc captures a high-level design for bringing the message-first architecture of `@monk-os-kernel` into the newer, more immature `abbot/` project.

The intent is a heavy refactor with no backwards-compat constraints.

## Goal

Recast Abbot as a small "kernel" that brokers all work as message streams, and as an intelligent proxy:

`OpenAI Provider` <-> `Abbot` <-> `Opencode CLI`

Abbot rewrites system prompts and tool exposure, merges internal + external tools, and routes tool execution via a syscall/sigcall-style infrastructure.

## What Monk OS Kernel Does (Relevant Pieces)

### Message-first core

- The fundamental unit is not "function returns value" but `Message -> AsyncIterable<Response>`.
- A syscall handler is an async generator that yields `Response` objects:
  - non-terminal: `item | data | event | progress`
  - terminal: `ok | error | done | redirect`

### Transport and dispatch

- Userspace processes send `syscall:request { id, pid, name, args }`.
- Kernel returns `syscall:response { id, result: Response }`.
- Dispatch is intentionally explicit (a `switch(name)`) with explicit dependencies.

### Backpressure as a protocol

- Streams are consumer-driven.
- Userspace periodically sends `syscall:ping { id, processed }`.
- Kernel pauses production at a high-water threshold and resumes when pings reduce the gap.
- Userspace can abort with `syscall:cancel { id }`.
- Kernel detects stalled consumers (no ping for a timeout) and aborts.

### SIGTICK

- A kernel signal (`SIGTICK`) broadcast on a timer.
- Subscription is opt-in (`proc:tick:subscribe` / `unsubscribe`).
- Payload includes `{ dt, now, seq }`.

### Sigcall (reverse syscall)

- Userspace can register handlers by name (`sigcall:register <name>`).
- When dispatch cannot resolve a syscall name, the kernel checks the registry and routes it to a userspace handler.
- Wire direction flips:
  - kernel -> userspace: `sigcall:request`
  - userspace -> kernel: `sigcall:response` (streamed)
- Registry is kernel-owned and cleaned up on process exit.

### Hard boundary: userspace vs kernel

- `src/` is kernel + dispatch + primitives.
- `rom/` is userspace libraries and programs.
- Userspace can only interact with kernel via the message protocol.

## Where Abbot Is Today

Abbot effectively has two messaging systems:

### 1) Bus (pub/sub + persistence)

- IRC-like scopes, broadcast fanout, UI feed.
- `tokio::broadcast` drops messages for lagging receivers.
- Great for audit logs and UI updates, weaker as the primary "kernel API".

### 2) KernelDispatcher (point-to-point syscall-ish)

- `Frame` protocol: `Req | Cancel | Ok | Error | Item | Bytes | Event | Progress`.
- Streaming uses `mpsc` and internal queue watermarks.
- Cancellation/deadlines exist via `CancellationToken` and `deadline_ms`.

### Existing sigcall-adjacent behavior

- OpenAI compat flow can emit `external_tool_request` as a tool-call-like event.
- Client returns tool output via `external_tool_result`.
- This is conceptually sigcall, but implemented ad-hoc at the chat layer, not as a first-class kernel facility.

## Proposed Direction: Monkify Abbot

### 1) Choose one kernel message protocol

- Unify around a single streaming response model.
- Make stream termination explicit (add a `Done`/`Redirect` equivalent if needed).
- Stop relying on implicit end-of-stream conventions.

### 2) Make backpressure part of the protocol

Abbot's current backpressure works in-process (receiver drains queue). Monk's model generalizes across boundaries.

- Add explicit `ping(processed)` and `cancel()` semantics for all streams.
- Use the same control plane for:
  - internal syscalls
  - upstream provider streaming (SSE)
  - external tool execution (Opencode)

### 3) Treat runtime services as "userspace processes"

Even if implemented as Rust tasks, enforce an OS-like contract:

- Each service has a `ProcessContext` (identity, scope/session, permissions, cwd-like state, active streams).
- Services communicate via kernel-managed channels/syscalls, not by reaching into shared state.

### 4) Replace most of the bus with point-to-point syscalls

Assumption: the MessageBus is largely replaced, with minor exceptions.

- Re-express Need/Goal/Task flows as explicit syscall endpoints:
  - `need:*` (enqueue/lease/ack/fulfill)
  - `task:*` (create/lease/progress/result/cancel)
- Keep a thin pub/sub layer only for:
  - UI/event fanout (websocket activity feed)
  - audit/persistence stream
  - coarse system events (e.g. reboot/health)

### 5) First-class sigcall for external tools (Opencode)

Make tool invocation a syscall:

- `tool:<name>(args) -> stream<Response>`

Routing:

- If tool is internal: run as kernel syscall handler.
- If tool is external (Opencode): route via sigcall:
  - Abbot -> Opencode: `sigcall:request` (transported via OpenAI tool_call)
  - Opencode -> Abbot: `sigcall:response` (transported via OpenAI tool result)

Registry scope:

- Sigcall registry is per-session (the existing `session/<hash>` notion is a good anchor).

This turns the current `external_tool_request` / `external_tool_result` convention into a kernel facility.

### 6) Port SIGTICK as a kernel signal

- Provide `tick:subscribe` / `tick:unsubscribe`.
- Deliver a tick payload (`dt, now, seq`).
- Use ticks for autonomy scheduling and uniform health/heartbeat.

## Proxy Topology: Provider <-> Abbot <-> Opencode

Think of Abbot as a microkernel sitting between:

- upstream model provider (OpenAI-compatible)
- Opencode CLI (external tool runner + UX)

Key idea:

- The transport (OpenAI HTTP + tool_calls) is analogous to Monk's gateway.
- Abbot owns the syscall namespace and tool registry.
- Sigcall provides the controlled escape hatch to the Opencode tool world.

## Phase 2 (Next Discussion)

When ready, translate this overview into concrete design artifacts:

- kernel protocol: frame/response types, terminal semantics, ping/cancel
- syscall namespace layout and ownership rules
- sigcall registry model (per-session, per-scope, lifetimes)
- which parts of the bus remain (UI/audit) and how they're fed
- migration plan for Need/Goal/Task to syscall endpoints
