# Abbot - Agent Guide

For AI agents working on this codebase.

## What Is Abbot?

Message-first microkernel daemon implementing distributed AI cognition:
- **OpenAI-compatible provider** - Clients talk to Abbot via `/v1/chat/completions`
- **Intelligent proxy** - Sits between upstream LLM providers and clients
- **Autonomous system** - Proactive Mind layer runs on SIGTICK

## Core Architecture

### Frames

All communication flows through `KernelDispatcher` via Frames:
- `id` / `parent_id` - Request/response correlation
- `op` - Req | Ok | Done | Error | Item | Progress | Cancel | Redirect
- `name` - Syscall name (e.g., "need:enqueue")
- `actor` - Scope/session identifier
- `data` - JSON payload

**Key principle**: Services never call each other directly. All interaction is via syscalls.

### Syscall Namespaces

- `need:*` - Work queue for Head (enqueue, lease, ack, fulfill)
- `task:*` - Work queue for Hand (enqueue, lease, progress, result, cancel)
- `room:*` - Deliberation rooms (create, join, propose, vote, close)
- `tick:*` - Timer signals (subscribe, unsubscribe)
- `log:*` - Audit/query (append, select)
- `reply:*` - Reply streams (send, close)

### Mind / Head / Hand

**Mind** (strategic): Subscribes to SIGTICK, creates needs, manages LTM + Self, deliberates in Conclave

**Head** (tactical): Leases needs, converts needs→tasks, manages STM, responds to users

**Hand** (operational): Leases tasks, executes tools via HAL, returns results as frames, no LLM calls

**Flow**: User message → `need:enqueue` → Head leases → `task:enqueue` → Hand executes → `reply:send`

## Memory Model

- **Self** - Collective identity. Managed by Conclave via proposals. Injected into all agent contexts.
- **LTM** - Long-term memory. Strategic learnings. Managed by Conclave. Flows into Head contexts.
- **STM** - Short-term memory. Working memory. Managed by Head. Flows into Hand contexts.

## EMS, VFS, HAL

- **EMS** - Schema-flexible SQLite entity store. Schema-on-write, all TEXT columns, JSON encoding.
- **VFS** - Mount-based filesystem isolation. No access without mounts. Longest-prefix match wins.
- **HAL** - Abstraction over host operations: HalFs, HalGit, HalNet, HalProcess.

Hand tools call HAL interfaces, never touch host filesystem directly.

## Frame Protocol

**Terminal frames** (end stream): Ok, Done, Error

**Non-terminal frames** (more may follow): Item, Bytes, Event, Progress

**Backpressure**: Producer pauses at high-water mark, resumes at low-water. Consumer can Cancel.

**Sigcall**: External tools routed via Redirect frames → transported as OpenAI tool_call → result returns via tool result.

## Constraints

Services MUST NOT: call other services directly, access filesystem outside VFS, block indefinitely, mutate shared state without syscalls.

Tools MUST: validate paths via VFS, enforce limits, return structured JSON, use HAL interfaces.

## Key Files

**Kernel**: `src/kernel/dispatcher.rs`, `src/kernel/frame.rs`, `src/kernel/router.rs`

**Runtime**: `src/runtime/kernel.rs`, `src/runtime/mind_service.rs`, `src/runtime/head_service.rs`, `src/runtime/hand_service.rs`, `src/runtime/conclave.rs`

**Layers**: `src/ems/service.rs`, `src/vfs/mount.rs`, `src/hal/fs.rs`, `src/hal/git.rs`, `src/hal/net.rs`

**Server**: `src/server/handler.rs`, `src/server/anthropic.rs`
