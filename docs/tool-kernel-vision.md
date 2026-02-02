#+#+#+#+#+#+#+#+#+#+#+#+#############################################
# Abbot Tool Kernel Vision
#+#+#+#+#+#+#+#+#+#+#+#+#############################################

Abbot already behaves like a small OS: Mind/Head/Hand coordinate intent, planning, and execution, and the daemon mediates access to the outside world.

This doc proposes a concrete next step: introduce a message-first "tool kernel" under all tool calls, borrowing the syscall/dispatch model from `monk-os-kernel`.

The goal is not to re-implement an OS. The goal is to make Abbot's effectful actions:
- safer (capability-gated, sandboxed)
- more portable (HAL abstraction)
- more observable (structured tracing and replay)
- more composable (tools become userland services)

## Why This

Tool calls are already a syscall boundary in practice:
- They cross trust boundaries (model -> host)
- They need policy (what is allowed, where)
- They need cancellation (model changes its mind)
- They need accounting (time, bytes, spawned processes)
- They need structured outputs for UIs and logs

Today those concerns tend to leak across tools and ad-hoc wrappers.
The tool kernel makes them first-class.

## Core Idea

Everything effectful flows through a single message protocol.

1) Hand emits `KernelReq` messages.
2) The kernel dispatches requests to syscall handlers.
3) Syscalls yield `KernelRes` messages as an async stream.
4) The caller acks progress for backpressure, and can cancel.

Higher-level tools become "userland": they compose syscalls.

## Design Goals

- Narrow waist: keep the syscall surface small and stable.
- Message-first: every result is a typed response message (including progress).
- Streaming by default: even "single-shot" syscalls can stream.
- Cancellation: deadlines and explicit cancel tokens.
- Backpressure: avoid unbounded buffering when the UI/model is slow.
- Capability security: least privilege grants per request / per tool.
- Portability: OS-specific details behind HAL.
- Determinism and audit: log a syscall trace for every run.

## Non-Goals (for now)

- Full process isolation / containers.
- Arbitrary user-provided code running in a sandbox.
- Network policy beyond allow/deny lists.
- Perfect replay of non-deterministic syscalls (network/time).

## Proposed Layers (Mapping to Abbot)

### 1) Kernel Message Protocol

Introduce an internal envelope, conceptually similar to monk's `syscall:req` / `syscall:res`.

Requests:
- `id` (correlation)
- `name` (e.g. `fs:read`, `proc:spawn`)
- `args` (validated inputs)
- `caps` (capability grants)
- `deadline_ms`
- `trace` metadata

Responses (streamed):
- `ok` (terminal)
- `error` (terminal, typed error code)
- `item` (stream item)
- `data` (binary chunk)
- `event` (structured notification)
- `progress` (structured progress)
- `done` (terminal stream end)

### 2) Dispatcher

Single dispatch entrypoint routes by syscall name.

Properties:
- explicit dependency injection into handlers (no global state)
- structured validation at syscall boundaries
- consistent error codes

### 3) Syscall Handlers (Privileged)

Handlers are the only code allowed to touch:
- host filesystem
- process spawning
- network
- git
- database

This is where policy enforcement, capability checks, and auditing live.

### 4) Userland Tools (Composed)

"Tools" become compositions of syscalls.
They do not call host APIs directly.

This provides:
- uniform sandboxing and policy
- unit testing of tools with a fake syscall layer
- consistent tracing

## Capability Model

Capabilities are explicit grants attached to each request.
They are checked by syscall handlers.

Example capability concepts:
- `cap.fs.read` scoped to VFS prefixes (e.g. `/workspace/**`)
- `cap.fs.write` scoped to VFS prefixes
- `cap.proc.spawn` scoped to allowlisted executables + working dirs
- `cap.net.fetch` scoped to allowlisted hosts

Default posture: deny by default; grant narrowly; escalate explicitly.

## VFS as the Sandboxing Boundary

Introduce a virtual filesystem namespace for Abbot.

Key properties:
- All file syscalls accept VFS paths (e.g. `/workspace/src/main.rs`).
- The VFS layer maps VFS paths to host paths via mounts.
- Mounts carry policy (readonly, allowlist patterns, size limits).

Example mount set:
- `host:<repo-root> -> /workspace` (rw)
- `host:/tmp -> /tmp` (rw)
- everything else unmapped by default

This makes it practical to "mount" specific external directories into Abbot's world without giving raw host access.

## EMS as the Unified State Store

Abbot already has history storage. EMS can become the general entity store for:
- tool execution records (inputs/outputs metadata, durations, touched paths)
- artifacts (generated files, indexes)
- mount/capability grants per session
- UI event streams (subscribe to execution progress)

EMS provides a consistent data model and streaming queries that complement the message-first runtime.

## HAL for OS Portability

Define platform-dependent operations behind a HAL interface.

Syscalls call HAL; tools do not.
Examples:
- process spawning differences
- filesystem quirks (permissions, symlinks)
- network stack differences
- path normalization

## Sigcalls for Extensions (Optional)

Borrow monk's "sigcall" idea to enable services that run outside the kernel:
- the kernel can route a syscall name to a registered handler
- the handler streams responses back

This supports long-running services such as:
- indexers
- language server-like analyzers
- repo watchers

Even without worker isolation, the sigcall API provides a clean plugin boundary.

## Observability and Audit

Every syscall should produce structured trace metadata:
- start/end timestamps
- input sizes / output sizes
- touched VFS paths
- spawned processes
- network destinations
- error codes

Store syscall traces in EMS for replay/debugging and for the frontend to render.

## Integration Plan (Incremental)

Phase 1: Kernel protocol + dispatcher
- implement the request/response streaming envelope
- route existing tool operations through syscalls

Phase 2: VFS mount layer
- introduce `/workspace` and `/tmp`
- enforce that file operations go through VFS

Phase 3: Capability enforcement
- attach caps to requests
- deny-by-default posture for sensitive ops

Phase 4: HAL abstraction
- move OS-specific logic behind HAL
- make syscalls portable

Phase 5: Optional sigcalls
- plugin/services registry
- routing of unknown syscall names to registered handlers

## Open Questions

- Capability granularity: per-request vs per-session; prefix ACLs vs pattern rules.
- Streaming semantics: which syscalls must stream vs may return single-shot.
- Tool compatibility: how to bridge existing tool call shape to streamed responses.
- Policy placement: dispatcher-level policy vs syscall-level policy.
- Replay: how much determinism is required and which syscalls are marked non-replayable.
