# Abbot - Agent Guide

This file is for agents making changes to Abbot itself.

## Mental Model

- Everything is a Frame. Services communicate only by sending syscall `Req` frames through `KernelDispatcher` and consuming streamed responses.
- `actor` is authorization identity. Mutations are generally restricted to actors with `head/` prefix (see `src/kernel/syscall.rs`).
- Long-poll syscalls (`need:lease`, `task:lease`) must not share the same lane lock as enqueue/complete (see `src/kernel/router.rs`).

## What To Read First

- `src/bin/abbot.rs`: CLI, daemon bootstrap, config defaults, frontend launch
- `src/runtime/kernel.rs`: kernel init (VFS auto-mount, dispatcher registration)
- `src/kernel/frame.rs`, `src/kernel/dispatcher.rs`: wire protocol + streaming/backpressure
- `src/syscalls/`: syscall catalog and payload shapes
- `src/runtime/head_service.rs`, `src/runtime/hand_service.rs`, `src/runtime/mind_service.rs`: the three role loops
- `src/server/openai.rs`, `src/server/handler.rs`: OpenAI-compatible ingress + reply streaming
- `src/agent_tools.rs`: internal tool implementations + policy gates

## Common Change Patterns

Add a syscall

- Implement `Syscall` in `src/syscalls/<area>.rs` and register it in `src/syscalls/mod.rs:register_all`.
- Decide lane routing (immediate vs need/task/room). If it can block, keep it off the lane locks.

Add or change an internal tool

- Tool schemas live under `src/tools/` (role-prefixed catalogs in `dispatch.rs`).
- Tool dispatch routes through `dispatch_tool()` → `tool_to_syscall()` → kernel dispatcher.

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
- Frame audit DB: `<workspace>/frames.db` (query via `abbot frames replay`)
- Kernel tap: `KERNEL_TAP_FRAMES=1` (and `KERNEL_TAP_ALL=1` for verbose)
