## Harness Error Prototype

This repo currently loses useful error context by flattening errors into strings
as they move up the stack (HAL -> harness -> syscall -> kernel logging/UI). A
general "HarnessError" pattern keeps errors structured and composable across
layers, and only stringifies at the very edge (CLI/log line), while still
emitting a stable machine-readable shape for replay/debugging.

### Core idea

Define a single top-level error type per subsystem (a "harness") that always
carries:

- `op`: what operation was being performed (e.g. `LlmChat`, `GitRun`, `FsWrite`)
- `ctx`: safe request context (e.g. provider/model/base_url, path, args); never secrets
- `kind`: a typed cause (`Transport`, `HttpStatus`, `Decode`, `Timeout`, `Cancelled`, `Misconfigured`, ...)
- `retry`: retry metadata (`attempt`, `max_attempts`, `retryable`, optional `backoff`)
- optionally `source`: underlying error for debugging/backtraces; do not rely on `Display` for UX

### Sketch (Rust)

```rust
#[derive(Debug, Clone)]
pub struct HarnessError<Op, Ctx, Kind> {
    pub op: Op,
    pub ctx: Ctx,
    pub kind: Kind,
    pub retry: RetryMeta,
}

#[derive(Debug, Clone, Default)]
pub struct RetryMeta {
    pub attempt: usize,
    pub max_attempts: usize,
    pub retryable: bool,
}

#[derive(Debug, Clone)]
pub enum LlmOp {
    Chat,
}

#[derive(Debug, Clone)]
pub struct LlmCtx {
    pub provider: String,
    pub model: String,
    pub base_url: String,
}

#[derive(Debug, Clone)]
pub enum LlmKind {
    Http { status: u16, body_snippet: String },
    Transport { message: String },
    Decode { message: String },
    Timeout,
    Cancelled,
    Misconfigured { reason: String },
}
```

### End-to-end application

- HAL (low-level) returns typed errors with raw fields (status, response text, etc.).
- Harness (retry/orchestration) wraps HAL errors into `HarnessError` and adds retry metadata;
  never reduces errors to `String`.
- Syscall/service boundary converts `HarnessError` into:
  - a concise human message (e.g. for `KernelError`), and
  - a structured event/frame payload (for `frames replay`) containing `op/ctx/kind/retry`.
- CLI/UI prints either the one-liner or, in verbose mode, the structured payload.

### Benefits

- Debuggability: logs and frame replays can include `provider`, `model`, `base_url`, `status` without guessing.
- Correctness: retry policy decisions key off `kind`/`retryable`, not parsed strings.
- Safety: context is explicitly chosen and safe-to-log (never API keys).
