# Plan: Eliminate `session/*` Scopes — All Chat Uses `#main`

## Context

Abbot runs as a single-user daemon. The `session/<hash>` scope concept was designed for multi-user server sessions, but since there's no reliable way to distinguish sessions across client connections, it adds complexity for no benefit. All user-facing LLM communication (outside rooms) should route through `scope = "main"`.

Internal scopes (`head/*/mail`, `head/*/stm`, `task/*`, etc.) are **not affected**.

## Changes

### 1. `daemon/src/server/session_scope.rs` — Remove session scope generation

- Delete `session_scope_from()`, `jwt_principal()`, `sha256_hex()`
- Delete their tests (`session_scope_deterministic`, `session_scope_varies_by_cwd`, `session_scope_varies_by_token`)
- Remove `use sha2::Digest` and `use base64::Engine` imports (only used by deleted functions)
- **Keep**: `bearer_token()`, `api_key_or_bearer()`, `extract_env_block()`, `extract_env_cwd()`, `extract_cwd_heuristic()`

### 2. `daemon/src/server/openai.rs` — Always scope to "main"

- Remove import of `session_scope_from` (keep `bearer_token`, `extract_env_block`, `extract_env_cwd`)
- Replace PHASE 3: SCOPE DERIVATION (lines 725-786) with:
  - Keep `is_loopback_peer()` check — reject non-loopback requests (unless they have a bearer token)
  - Accept bearer tokens, log them, but don't use for scoping
  - Remove `contains_opencode_marker()` function and its usage
  - Remove `allow_loopback_main_scope` config usage
  - Always set `scope = "main"`
  - Still extract cwd from env block for `session_env` persistence
- Keep `ConnectInfo` extractor for the loopback check
- Update doc comments

### 3. `daemon/src/server/anthropic.rs` — Always scope to "main"

- Remove import of `session_scope_from` (keep `api_key_or_bearer`, `extract_cwd_heuristic`)
- Replace PHASE 1: SCOPE DERIVATION (lines 344-379) with:
  - Keep loopback / bearer token check — reject non-loopback unauthenticated requests
  - Accept bearer tokens, log them, but don't use for scoping
  - Always set `scope = "main"`
  - Still extract cwd via heuristic for `session_env` persistence
- Remove `is_localhost_request()` — use `is_loopback_peer()` pattern from openai.rs instead (or keep as-is if it works differently)
- Update doc comments

### 4. `daemon/src/server/ingress_hub.rs` — Tighten validation

- Change `is_valid_chat_scope()` to only accept `"main"`:
  ```rust
  fn is_valid_chat_scope(scope: &str) -> bool {
      scope.trim() == "main"
  }
  ```
- Update error messages: `"must be 'main'"` instead of `"must be 'main' or 'session/<hash>'"`
- Update doc comments

### 5. `daemon/src/kernel/frame_store.rs` — Remove session actor fallback

- In `extract_index_fields()` (lines 237-243), delete the block:
  ```rust
  if scope.is_none() {
      scope = frame.actor.as_deref()
          .filter(|s| s.starts_with("session/"))
          .map(|s| s.to_string());
  }
  ```

### 6. `daemon/src/runtime/app_config.rs` — Remove `allow_loopback_main_scope`

- Remove `pub allow_loopback_main_scope: Option<bool>` from `ServerToml`

### 7. `daemon/src/server/websocket.rs` — Force scope to "main"

- In the `ChatSend` match arm, ignore client-provided scope and hardcode `"main"`:
  ```rust
  Ok(WsInMessage::ChatSend { scope: _, text, id }) => {
      handle_chat_send(&state, &out_tx, &mut active_turns, "main".to_string(), text, id).await;
  }
  ```

### 8. Config files — Remove `allow_loopback_main_scope`

- `first-run.sh` — remove the line setting `allow_loopback_main_scope`
- `cli/src/config.rs` — remove from config template

### 9. `daemon/src/syscalls/session/mod.rs` + `model_set.rs` — Update comments

- Remove references to `"session/<id>"` in doc comments
- Replace with `"main"` where appropriate

### 10. Web frontend cleanup (dead code)

- `web/src/state.rs` — Remove `SessionChat` variant from `TabType`, `Tab::session_chat()`, `open_session_chat()` (all dead code)
- `web/src/components/frame_inspector.rs` — Simplify `format_scope()` to remove `session/` prefix check
- `web/src/components/frame_timeline.rs` — Remove `session/` prefix display logic

## Files NOT changed

- `scope.rs` — No session constructor exists
- `session_locks.rs` — Still useful for "main" scope serialization
- `history/store.rs` — Session tables work fine with `scope="main"`
- `kernel/external_tools.rs` — Scope-keyed storage harmless, works with "main"
- `runtime/head/mod.rs` — Already uses `vec![Scope::main()]`
- `runtime/head/bundle.rs` — Queries by scope, works with "main"
- `runtime/head/think.rs` — Uses default_scope from need, works with "main"
- `server/handler.rs` — Receives scope from caller, works with "main"
- `kernel/sigcall_hub.rs` — Tags frames with scope string, works with "main"

## Verification

1. `cargo fmt` then `cargo check` — no compile errors
2. `cargo clippy -- -D warnings` — clean
3. `cargo test` — all tests pass (deleted session_scope tests, remaining tests unaffected)
4. Manual: WebSocket chat sends scope="main" and works
5. Manual: OpenAI-compat API from localhost works with scope="main"
