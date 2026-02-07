## Harness Error Prototype

This repo currently loses useful error context by flattening errors into strings
as they move up the stack (HAL -> harness -> syscall -> kernel logging/UI). A
structured `HarnessError` keeps errors typed and composable across layers, and
only stringifies at the very edge (CLI/log line), while emitting a stable
machine-readable shape for replay/debugging.

### Where context is lost today

1. **HAL constructors** flatten immediately:
   `HalFsError::io(e)` calls `e.to_string()` (hal/fs.rs:47, hal/net.rs:89,97,
   hal/process.rs:84). The underlying `io::Error` kind is gone.
2. **LLM provider errors** store `message: String` instead of the source error
   (hal/llm/anthropic.rs:329,350).
3. **Retry harness** downcasts `Box<dyn Error>` to six provider-specific types
   via a long if-else chain (llm_harness.rs:143-244), then collapses the result
   to `HarnessError { message: String }` — all type info lost.
4. **Syscall boundary** uses `map_err(|e| KernelError::io(format!(...)))` 30+
   times. Worst case: fs/read.rs:254 matches on `e.to_string().contains("No
   such file")`.

### Core idea

Replace `HarnessError { message: String }` with a concrete enum that always
carries:

- **kind**: a typed cause (`Http`, `Transport`, `Decode`, `Timeout`,
  `Cancelled`, `Misconfigured`)
- **ctx**: safe request context (provider, model, base_url, path, args; never
  secrets)
- **retry**: retry metadata (`attempt`, `max_attempts`, `retryable`)

### Design: concrete enum (not generics)

The codebase has exactly one harness today (LLM). FS and Exec errors go straight
from HAL enums to KernelError. A concrete enum avoids monomorphization cost and
keeps `match` exhaustive:

```rust
/// Returned by the retry harness — replaces `HarnessError { message: String }`.
#[derive(Debug, Clone)]
pub struct HarnessError {
    pub kind: HarnessKind,
    pub ctx: HarnessCtx,
    pub retry: RetryMeta,
}

#[derive(Debug, Clone)]
pub enum HarnessKind {
    Http { status: u16, body: String },
    Transport { message: String },
    Decode { message: String },
    Timeout,
    Cancelled,
    Misconfigured { reason: String },
}

#[derive(Debug, Clone)]
pub struct HarnessCtx {
    pub provider: String,
    pub model: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RetryMeta {
    pub attempt: usize,
    pub max_attempts: usize,
    pub retryable: bool,
}
```

### Boundary conversions

#### HAL -> HarnessError (inside retry loop)

HAL errors stay as-is (`AnthropicHttpError`, `OpenAICompatTransportError`, etc).
The retry harness converts them into `HarnessError` instead of flattening to
String. This replaces the current downcast chain with a single conversion point:

```rust
fn from_hal_error(e: &(dyn Error + Send + Sync), ctx: &HarnessCtx) -> HarnessKind {
    // OpenAI
    if let Some(h) = e.downcast_ref::<OpenAICompatHttpError>() {
        return HarnessKind::Http { status: h.status, body: h.response_text.clone() };
    }
    if let Some(t) = e.downcast_ref::<OpenAICompatTransportError>() {
        return HarnessKind::Transport { message: t.message.clone() };
    }
    if let Some(d) = e.downcast_ref::<OpenAICompatDecodeError>() {
        return HarnessKind::Decode { message: d.message.clone() };
    }
    // Anthropic
    if let Some(h) = e.downcast_ref::<AnthropicHttpError>() {
        return HarnessKind::Http { status: h.status, body: h.response_text.clone() };
    }
    if let Some(t) = e.downcast_ref::<AnthropicTransportError>() {
        return HarnessKind::Transport { message: t.message.clone() };
    }
    if let Some(d) = e.downcast_ref::<AnthropicDecodeError>() {
        return HarnessKind::Decode { message: d.message.clone() };
    }
    // Unknown
    HarnessKind::Transport { message: e.to_string() }
}
```

The retry decision moves from scattered if-else to a single match:

```rust
fn is_retryable(kind: &HarnessKind) -> bool {
    match kind {
        HarnessKind::Http { status, .. } => matches!(status, 429 | 500 | 502 | 503 | 504),
        HarnessKind::Transport { .. } => true,
        HarnessKind::Decode { .. } => true,
        HarnessKind::Timeout => true,
        HarnessKind::Cancelled => false,
        HarnessKind::Misconfigured { .. } => false,
    }
}
```

#### HarnessError -> KernelError (at syscall boundary)

`KernelError` already has `detail: Option<Value>` and `retryable: Option<bool>`.
The conversion populates both:

```rust
impl From<HarnessError> for KernelError {
    fn from(e: HarnessError) -> Self {
        let (code, message) = match &e.kind {
            HarnessKind::Http { status, .. } => {
                ("E_IO", format!("{} returned HTTP {status}", e.ctx.provider))
            }
            HarnessKind::Transport { message } => {
                ("E_IO", format!("{}: {message}", e.ctx.provider))
            }
            HarnessKind::Decode { message } => {
                ("E_IO", format!("{}: decode error: {message}", e.ctx.provider))
            }
            HarnessKind::Timeout => {
                ("E_TIMEOUT", format!("{} request timed out", e.ctx.provider))
            }
            HarnessKind::Cancelled => {
                ("E_CANCELLED", format!("{} request cancelled", e.ctx.provider))
            }
            HarnessKind::Misconfigured { reason } => {
                ("E_INVALID_ARGS", format!("{}: {reason}", e.ctx.provider))
            }
        };

        KernelError::new(code, message)
            .with_retryable(e.retry.retryable)
            .with_detail(serde_json::json!({
                "provider": e.ctx.provider,
                "model": e.ctx.model,
                "base_url": e.ctx.base_url,
                "kind": format!("{:?}", e.kind),
                "attempt": e.retry.attempt,
                "max_attempts": e.retry.max_attempts,
            }))
    }
}
```

This means `syscalls/llm/chat.rs` goes from:

```rust
Err(e) => Err(KernelError::internal(e.message))
```

to:

```rust
Err(e) => Err(KernelError::from(e))
```

### Frame payload

The structured detail lands in `KernelError.detail` as JSON. When the syscall
emits a `Frame::err`, the frame store persists the full detail object. A
`frames:select` replay can then filter/display by provider, status, model, etc.
without parsing log strings.

Example stored detail:

```json
{
  "provider": "anthropic",
  "model": "claude-sonnet-4-5-20250929",
  "base_url": "https://api.anthropic.com",
  "kind": "Http { status: 529, body: \"overloaded\" }",
  "attempt": 4,
  "max_attempts": 5
}
```

### What stays the same

- **HAL error enums** (`HalFsError`, `HalNetError`, `HalProcessError`,
  `Anthropic*Error`, `OpenAICompat*Error`) — unchanged. They are internal to the
  HAL layer.
- **KernelError** — unchanged. It gains richer `detail` payloads but the struct
  is the same.
- **RetryPolicy** — unchanged. `HarnessError` carries the outcome; the policy
  still drives the loop.

### What changes

| File | Change |
|---|---|
| `runtime/llm_harness.rs` | Replace `HarnessError { message }` with structured `HarnessError { kind, ctx, retry }`. Replace downcast if-else chain with `from_hal_error()` + `is_retryable()`. |
| `syscalls/llm/chat.rs` | Replace `KernelError::internal(e.message)` with `KernelError::from(e)`. |

### Out of scope (for now)

- **FS / Exec / Net harness errors**: these subsystems don't have a retry
  harness. Their HAL→KernelError conversions in syscalls are adequate. If a retry
  harness is added for net/fetch later, the same pattern applies.
- **Replacing HAL error enums**: they work fine internally. The harness boundary
  is where context gets lost.
- **`thiserror` / `anyhow`**: the codebase uses manual error types. No reason to
  add a dependency for this change.

### Benefits

- **Debuggability**: frame replays include provider, model, base_url, status
  without guessing.
- **Correctness**: retry decisions key off `HarnessKind` / `is_retryable()`, not
  `downcast_ref` chains or parsed strings.
- **Safety**: context is explicitly chosen and safe-to-log (never API keys).
- **Minimal blast radius**: two files change. HAL and KernelError are untouched.
