# Syscalls

Syscalls are the fundamental operations available to agents. Each syscall has a namespaced name (e.g., `fs:read`) and accepts a JSON payload.

## Namespaces

| Namespace | Purpose |
|-----------|---------|
| `fs:*` | Filesystem operations (read, write) |
| `git:*` | Git operations |
| `llm:*` | LLM provider interactions (chat, chaos) |
| `chat:*` | Chat message routing |
| `frames:*` | Frame store queries |
| `task:*` | Task queue management |
| `room:*` | Room lifecycle (create, run, stream) |
| `need:*` | Need/want management |
| `net:*` | Network fetch |
| `proc:*` | Process execution |
| `tick:*` | Tick/heartbeat |
| `tool:*` | External tool management |
| `docs:*` | Documentation access |

## Response Pattern

Syscalls communicate via frames:

- `Frame::ok(id, data)` — Success with payload.
- `Frame::error(id, data)` — Failure with error details.
- `Frame::item(id, data)` — Streaming result item (for multi-result syscalls).
- `Frame::done(id)` — End of stream marker.
