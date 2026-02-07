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

- Tool schemas live under `src/tools/` (role-prefixed `head__*`, `hand__*`, `mind__*`).
- Tool execution lives in `src/agent_tools.rs`.
- Keep hands read-only: enforce policy in the tool gate, not only by prompt wording.

Add a plugin tool

- Add a new directory under `src/plugins/<id>/` with `plugin.toml` and optional `head.md`/`hand.md`.
- Plugins are compiled into the binary via `build.rs` and enabled via `[plugins]` in `~/.abbot/abbot.toml`.

Work on external tool support

- External tools are registered per scope via `tool:register` and exposed to heads as `user__<name>`.
- Calling `user__*` emits a terminal Redirect into the reply stream; clients resume the need by submitting `role:"tool"` messages (handled by `tool:result`).

## Guardrails

- This repository is alpha-stage (v0.1.x). Breaking config changes are acceptable.
  Assume users may frequently delete `~/.abbot/` between runs; prioritize simple,
  deterministic startup behavior over backwards-compatible config parsing.

- Do not bypass VFS/HAL for filesystem/process/network work; host access must be mediated.
- Do not add new cross-service calls; add syscalls instead.
- Keep `/admin/*` localhost-only; treat it as trusted UI plumbing for the TUI/web.

## Observability

- WebSocket frame stream: `/ws` (used by `abbot monitor` and `abbot-tui`)
- Frame audit DB: `<workspace>/logs.db` (query via `abbot frames replay`)
- Kernel tap: `KERNEL_TAP_FRAMES=1` (and `KERNEL_TAP_ALL=1` for verbose)
