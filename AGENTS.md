# Abbot - Agent Guide

This file is for agents making changes to Abbot itself.

## Mental Model

- Everything is a Frame. Services communicate only by sending syscall `Req` frames through `KernelDispatcher` and consuming streamed responses.
- `actor` is attribution + policy identity. Mutations are generally restricted to trusted actors (today: `head/` and `mind/`; see `daemon/src/kernel/syscall.rs`).
- Long-poll syscalls (notably `need:lease`) must not share the same lane lock as enqueue/fulfill to avoid deadlock (see `daemon/src/kernel/router.rs`).

## What To Read First

- `daemon/src/bin/abbot.rs`: abbotd entrypoint (daemon bootstrap + frontend launch)
- `daemon/src/runtime/kernel.rs`: kernel init (VFS mounts, dispatcher registration)
- `daemon/src/kernel/frame.rs`, `daemon/src/kernel/dispatcher.rs`, `daemon/src/kernel/router.rs`: wire protocol + streaming/backpressure + lane routing
- `daemon/src/syscalls/`: syscall catalog and payload shapes
- `daemon/src/runtime/head/mod.rs`, `daemon/src/runtime/hand/mod.rs`, `daemon/src/runtime/mind/service.rs`, `daemon/src/runtime/room/runner.rs`: the role loops + room orchestration
- `daemon/src/server/openai.rs`, `daemon/src/server/handler.rs`: OpenAI-compatible ingress + reply streaming
- `daemon/src/syscalls/dispatch.rs`: tool name → syscall mapping and tool dispatch

## Common Change Patterns

Add a syscall

- Implement `Syscall` in `daemon/src/syscalls/<area>/...` and register it in `daemon/src/syscalls/mod.rs:register_all` (or the namespace `register()` helper).
- Decide lane routing (immediate vs need/room). If it can block, keep it off the lane locks.

Add or change an internal tool

- Tool schemas are JSON co-located with syscalls (`daemon/src/syscalls/**.json`) plus room/hand helpers under `daemon/src/runtime/room/*.json`.
- Tool dispatch routes through `dispatch_tool()` → `tool_to_syscall()` → kernel dispatcher (see `daemon/src/syscalls/dispatch.rs`).

Work on external tool support

- External tools are registered per room via `tool:register` and exposed to heads as `user__<name>`.
- Calling `user__*` emits a terminal Redirect into the reply stream; clients resume the need by submitting `role:"tool"` messages (handled by `tool:result`).
- The Door trait mediates external tool delivery; TurnRuntime handles the rendezvous between waiter and deliverer.

## Testing Boundaries

Tests are organized around two assumption borders — mock at the border, trust everything behind it.

**Door border** (runner tests)

The Door trait is the boundary between the agent runner and all tool dispatch. MockDoor lets tests control what every tool call returns — internal (`fs:list`, `ems:query`) or external (`user__*`) — without a filesystem, database, or LLM. Use this layer to test runner orchestration: tool routing, result injection, round lifecycle, termination conditions.

**Kernel services border** (syscall tests)

Each syscall's `execute()` runs against real kernel services (Store, FrameStore, EMS, TurnRuntime). Tests at this layer verify that a syscall produces the correct mutations — frames appended, entities written, turn state changed — given controlled inputs. No runner or LLM involved.

**Unit tests** (service internals)

Pure unit tests on individual kernel services (e.g., TurnRuntime rendezvous, FrameStore persistence). No syscall dispatch, no runner.

The rule: mock at the border you're testing against, use real implementations below it. Runner tests mock the Door. Syscall tests use real services. Service tests use nothing external.

## Guardrails

- This repository is alpha-stage (v0.1.x). Breaking config changes are acceptable.
  Assume users may frequently delete `~/.abbot/` between runs; prioritize simple,
  deterministic startup behavior over backwards-compatible config parsing.

- Do not bypass VFS/HAL for filesystem/process/network work; host access must be mediated.
- Do not add new cross-service calls; add syscalls instead.
- Keep `/admin/*` localhost-only; treat it as trusted UI plumbing for the TUI/web.

## Observability

- WebSocket frame stream: `/ws` (used by `abbot monitor` and `abbot-tui`)
- Frame audit DB: `~/.abbot/frames.db` (query via `abbot frames replay`)
- Kernel tap: `KERNEL_TAP_FRAMES=1` (and `KERNEL_TAP_ALL=1` for verbose)
