# Monkified Abbot (Current Architecture)

This document describes the current "monkified" direction of Abbot: a message-first microkernel with point-to-point syscalls, streaming responses, explicit backpressure, and sigcall-style external tool bridging.

It is intended to become the frame for a future rewrite of the main README.

## What Changed (High-Level)

Abbot is moving away from a lossy broadcast MessageBus as the primary control plane.

Instead:

- The kernel owns the primary message protocol (`Frame`) and dispatch (`KernelDispatcher`).
- Most coordination happens through point-to-point syscalls that return streaming frames.
- Backpressure is enforced centrally in the kernel.
- External tools (Opencode) are treated like userspace handlers (sigcall-like) rather than special-case chat events.

The MessageBus remains for a shrinking set of UI/audit/event use-cases, but core flows are migrating off it.

## Kernel Message Protocol

### Frame

All kernel interactions use `Frame`:

- Request: `FrameOp::Req` with `{ name, data }`
- Stream outputs:
  - `Bytes` (text chunks)
  - `Item` (records)
  - `Event` (typed events)
  - `Progress`
- Terminal:
  - `Ok` / `Error`
  - `Done` (explicit end)
  - `Redirect` ("I can't help you here; run this elsewhere")

Key rule: terminal frames end the stream deterministically.

Relevant code:

- `abbot/src/kernel/frame.rs`

### Backpressure

Kernel dispatch streaming is consumer-driven:

- The dispatcher enforces high/low watermarks and cancels stalled streams.
- Local consumers auto-ack on `KernelReceiver::recv()`.

Relevant code:

- `abbot/src/kernel/dispatcher.rs`

## Kernel Router + Lanes

Syscalls are routed into execution lanes by prefix:

- `need:*` -> `Lane::Need`
- `task:*` -> `Lane::Task`
- `room:*` and room-adjacent `mind:*` -> `Lane::Room`
- everything else -> `Lane::Immediate`

Lanes are where special queuing / serialization rules live. This is the single place to encode "task calls are scheduled differently" vs "everything else".

Relevant code:

- `abbot/src/kernel/router.rs`
- `abbot/src/kernel/dispatcher.rs`

## Core Kernel-Owned Subsystems

### Reply Streams (transport -> head output)

The chat transport no longer tails `bus.subscribe_all()` to stream responses.

Instead, the kernel owns per-thread reply streams keyed by `(scope, reply_to)`.

- Server opens a reply stream for a user message.
- Heads emit `Bytes` / `Redirect` / `Done` frames into that reply stream.

Relevant code:

- `abbot/src/kernel/reply_streams.rs`
- `abbot/src/server/handler.rs`

### Needs

Needs are kernel-owned, not bus-owned:

- `need:enqueue`
- `need:lease`
- `need:fulfill`

Heads lease needs directly from the kernel rather than receiving a bus dispatch message.

Relevant code:

- `abbot/src/kernel/needs.rs`
- `abbot/src/syscalls/need.rs`
- `abbot/src/runtime/head_service.rs`

### Tasks

Tasks are kernel-owned, not bus-owned:

- `task:enqueue`
- `task:lease`
- `task:complete`
- `task:status`

Hands lease tasks from the kernel and complete them via syscalls.
Heads wait on kernel task status/watchers rather than bus task result events.

Relevant code:

- `abbot/src/kernel/tasks.rs`
- `abbot/src/syscalls/task.rs`
- `abbot/src/runtime/hand_service.rs`
- `abbot/src/runtime/head_service.rs`

### Rooms (Conclave + Autonomy)

Rooms are a kernel primitive:

- `room:create` creates a room and returns `room_id`.
- `room:stream` opens a point-to-point stream of room frames.
- `room:run` runs the room and publishes room lifecycle frames into the room stream.

Room scheduling is isolated in `Lane::Room`.

Relevant code:

- `abbot/src/kernel/rooms.rs`
- `abbot/src/syscalls/room.rs`

## Mind Syscalls

Mind triggers are moving from bus events to syscalls:

- `mind:convene_conclave`
- `mind:convene_autonomy`

These run the existing `Conclave` engine under kernel control.

Relevant code:

- `abbot/src/syscalls/mind.rs`
- `abbot/src/runtime/conclave.rs`

## External Tools (Sigcall-Style)

External tool execution is kernel-owned:

- Tool registry is stored per session.
- Tool results are delivered directly to the kernel.
- Heads emit a terminal `Redirect` frame to ask the client (Opencode) to run a tool.

This replaces bus-level `external_tool_request` / `external_tool_result` patterns.

Relevant code:

- `abbot/src/kernel/external_tools.rs`
- `abbot/src/server/openai.rs`
- `abbot/src/runtime/head_service.rs`

## Kernel Store Attachment

The kernel is long-lived and needs access to persisted state.

- The daemon attaches `Store` to the kernel after opening SQLite.
- Room execution uses this store to persist transcripts/decisions.

Relevant code:

- `abbot/src/runtime/kernel.rs`
- `abbot/src/bin/abbot.rs`

## Design Principles (Monk OS Inspired)

- Message-first: everything is a stream of frames.
- Backpressure is enforced centrally.
- Syscalls are explicit; broadcast is a last resort.
- Userspace-style components (heads/hands/opencode tools) interact via kernel-managed protocols.
- Redirect is a first-class terminal outcome: "try over here".
