## External Mounts

This document describes how Abbot can mount a user-provided filesystem (e.g. an OpenCode client’s CWD) into Abbot’s VFS and proxy file operations via external tool calls.

The core idea is:

- VFS remains the path routing layer.
- Kernel syscalls remain the policy boundary (scope, mutation, deadlines, cancellation).
- External mounts introduce a second filesystem backend that executes operations by RPC/tool-call to an external client.

This enables multiple concurrent OpenCode sessions without leaking host filesystem access into Abbot.

---

## Goals

- Mount a client-side directory ("/home") into Abbot’s VFS.
- Allow both heads and hands to read external files.
- Enforce that hands are always read-only, including on external mounts.
- Support multiple concurrent OpenCode sessions with correct request routing.

## Non-Goals

- Making external mounts the primary workspace.
- Allowing hands to write anywhere.
- Encoding session routing into VFS paths.

---

## Definitions

- **Workspace**: Abbot’s configured private directory on the host (contains `root/` and private DBs/state).
- **VFS**: Virtual filesystem with mount table mapping VFS prefixes to backends.
- **Kernel syscalls**: The unified execution surface (fs/proc/net/git) that enforces policy.
- **Scope**: Authorization label (e.g. `head/<id>`, `hand/<id>`). Hands must never be able to mutate.
- **Session ID**: A routing key that identifies the external client session. For OpenCode, this is `session/<hash>` derived from `hash(auth.sub + user.cwd)`.

---

## Architectural Overview

### Why this can’t be a simple host mount

Current VFS mounts map to a local host path. For external mounts, Abbot cannot assume it has direct filesystem access to the user’s machine.

Therefore, an external mount must resolve to a non-host backend and delegate I/O through an external RPC/tool-call.

### Session-scoped routing

Multiple OpenCode sessions can be active simultaneously.

To avoid embedding session IDs into paths (e.g. `/<session>/home/...`), Abbot routes by a dedicated `session_id` field carried in kernel requests.

VFS paths remain stable (e.g. `/home/project/file.rs`), while the mount table and external RPC target are selected by `session_id`.

---

## Mount Model

### Mount kinds

Mounts must support multiple backend kinds:

- `host`: a local host path (current behavior)
- `external`: a client-backed filesystem accessed via RPC/tool calls

### Default mounts per session

For every active session, Abbot should expose at least:

- `/` -> `<workspace>/root` (host backend)
- `/home` -> external backend (OpenCode session)

Mount mode:

- `/home` is effectively RW-capable so heads can write.
- Hands remain read-only regardless of mount mode (policy enforced by kernel scope).

---

## Kernel Request Context

Kernel requests must carry two distinct pieces of metadata:

1) **Authorization scope**

- `head/<head_id>`
- `hand/<hand_id>`

This drives mutation control (`hand/*` cannot mutate).

2) **Session routing key**

- `session/<hash>`

This selects:

- which mount table to use
- which external client connection to send proxy requests to

This must not be overloaded into `scope`.

---

## Syscall Behavior

### Path semantics

- Syscalls that accept paths require absolute VFS paths starting with `/`.
- Tool-layer code converts user-relative paths into absolute VFS paths.

### Backend selection

`fs:*` syscalls resolve VFS paths using the mount table selected by `session_id`:

- If the resolved mount backend is `host`, perform local I/O.
- If the resolved mount backend is `external`, issue an external RPC/tool call.

### Mutation control

- Any mutating syscall (e.g. `fs:write`) must call `ctx.require_mutation()`.
- Since hands always have `hand/<id>` scope, writes are rejected everywhere, including external mounts.

Optional defense-in-depth:

- External RPC layer also rejects writes unless caller scope starts with `head/`.

---

## External RPC / Tool Proxy

### Requirements

External proxying must be a real request/response mechanism:

- correlation IDs
- concurrent in-flight calls
- timeouts/deadlines
- cancellation propagation
- bounded output sizes and structured errors

The existing `external_tool_request`/`external_tool_result` mechanism in `src/runtime/head_service.rs` is LLM-loop oriented (pause/resume a need) and is not sufficient as a general syscall backend.

### Suggested surface

Define an internal service like:

```text
ExternalRpc::call(session_id, name, args_json, deadline, cancel) -> Result<output_json>
```

The OpenCode transport must handle receiving these requests and executing the corresponding client-side tools.

---

## Session Lifecycle

### Mount registration

On OpenCode session start (or first message), Abbot registers/updates mounts for that `session_id`:

- Ensure `/` mount exists (host workspace root).
- Ensure `/home` mount exists (external backend for that session).

### Cleanup

When a session disconnects:

- Remove its mount table entry.
- Cancel any in-flight external RPC calls for that session.

---

## Security Notes

- External mounts are not a privilege escalation path if:
  - kernel mutation control is enforced by scope
  - VFS only exposes paths via mounts
  - external RPC calls are routed strictly by session_id
- Do not allow a session to affect another session’s mount table.
- Avoid reflecting untrusted paths directly into host paths; external mounts should never map to host paths.

---

## Implementation Notes (High-Level)

- Replace global `MountTable::global()` (`OnceLock`) with per-session mount tables owned by the `Kernel` runtime.
- Extend kernel request frames/context to include `session_id`.
- Update `agent_tools` -> syscall wiring to populate `scope` and `session_id`.
- Implement an external filesystem backend that calls OpenCode via RPC.
