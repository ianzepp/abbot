# Pipeline Optimizations

Tracking document for Need → Head → Hand → Tool execution path optimizations.

## High Impact (Latency)

### 1. TaskService 100ms Polling Loop
- [x] **Status:** Complete
- **Location:** `src/runtime/task_service.rs:144-148`
- **Problem:** Dispatch loop sleeps 100ms between iterations regardless of work availability. Every task dispatch has 0-100ms latency.
- **Fix:** Add `dispatch_notify: Arc<Notify>` and wake on task enqueue or hand idle (same pattern as NeedService lines 57, 135, 219, 308, 391).
- **Impact:** -50ms average dispatch latency

### 2. 5ms Sleep Between Tool Calls
- [x] **Status:** Complete
- **Location:** `src/runtime/hand_service.rs:442`
- **Problem:** Artificial 5ms delay after every tool execution. 10 tools = 50ms wasted.
- **Fix:** Remove entirely. Tokio yields naturally on `.await`.
- **Impact:** -5ms per tool call

### 3. Synchronous SQLite + Write Lock on Every Publish
- [x] **Status:** Complete (async spawn for SQLite; write lock retained for Hub)
- **Location:** `src/runtime/bus.rs:34-39`
- **Problem:** Every message blocks on disk I/O (`store.insert`) then acquires exclusive write lock for broadcast.
- **Fix:**
  - Spawn persistence to background task (fire-and-forget or buffered)
  - Use `RwLock::read()` for broadcast (channels are internally synchronized)
- **Impact:** Unblock hot path, reduce lock contention

---

## Medium Impact (Redundant Work)

### 4. Redundant Need Dispatch Messages
- [x] **Status:** Complete
- **Location:** `src/runtime/need_service.rs:266-289`
- **Problem:** Publishes TWO messages when dispatching a need:
  1. `NeedMsg::Acknowledged` (line 267) - just logged and skipped by head
  2. Chat with string-formatted content (line 284)
- **Fix:** Single `NeedMsg::Dispatch { need_id, source, priority, need, context, scope }` variant.
- **Impact:** -2 SQLite writes per need, simpler flow

### 5. String Parsing for Need Content
- [x] **Status:** Complete (eliminated by #4)
- **Location:** `src/runtime/head_service.rs:419-462`
- **Problem:** Need content formatted as string `[need_id=X] [source=Y]...` then parsed back with brittle string operations.
- **Fix:** Use structured `NeedMsg::Dispatch` variant with typed fields (depends on #4).
- **Impact:** CPU savings, eliminates parsing bugs

### 6. Duplicate Tool Progress Messages
- [x] **Status:** Complete
- **Location:** `src/runtime/hand_service.rs:350-374`
- **Problem:** Publishes BOTH `TaskProgress` AND `TaskToolCall` for every tool invocation.
- **Fix:** Remove `TaskProgress`, keep only `TaskToolCall` (or merge fields).
- **Impact:** -1 message + SQLite write per tool call

### 7. External Tools Rebuilt Per Need
- [x] **Status:** Complete
- **Location:** `src/runtime/head_service.rs:566-579`
- **Problem:** Queries DB and rebuilds external tool list for every need processed.
- **Fix:** Cache external tools in `SnapshotManager`, refresh on tool registration events.
- **Impact:** -1 DB query per need, fewer allocations

---

## Lower Impact (Allocations)

### 8. Priority Queue Sort on Every Insert
- [x] **Status:** Complete
- **Location:** `src/runtime/need_service.rs:196-202`
- **Problem:** Full `queue.sort_by()` on every need insert - O(n log n).
- **Fix:** Use `std::collections::BinaryHeap<Need>` with custom `Ord` impl - O(log n) insert.
- **Impact:** Better scaling with queue depth

### 9. Tools/Messages Cloned on Every LLM Attempt
- [ ] **Status:** Not started
- **Locations:**
  - `src/runtime/llm_harness.rs:67`
  - `src/runtime/head_service.rs:589`
  - `src/runtime/hand_service.rs:253`
- **Problem:** `messages.clone()` and `tools.clone()` on every LLM call attempt, including retries.
- **Fix:** Take `&[ToolSpec]` or `Arc<Vec<ToolSpec>>` to avoid cloning. Only clone messages when modifying.
- **Impact:** Fewer allocations in hot path

### 10. Multiple Lock Acquisitions in try_dispatch
- [x] **Status:** Complete
- **Location:** `src/runtime/task_service.rs:231-286`
- **Problem:** Acquires locks 3 separate times per dispatch:
  - Line 233: `hands` (find idle)
  - Lines 243-244: `rr_scopes` + `queues` (pop task)
  - Line 278: `hands` again (update state)
- **Fix:** Single unified state struct or acquire all locks atomically once.
- **Impact:** Reduced lock contention

---

## Implementation Priority

| Priority | Item | Effort | Impact |
|----------|------|--------|--------|
| 1 | #1 TaskService polling → Notify | Low | High |
| 2 | #2 Remove 5ms tool sleep | Trivial | Medium |
| 3 | #4+#5 Combine dispatch messages | Medium | Medium |
| 4 | #6 Remove duplicate progress msgs | Low | Low |
| 5 | #3 Async SQLite persist | Medium | High |
| 6 | #8 BinaryHeap for needs | Low | Low |
| 7 | #7 Cache external tools | Medium | Medium |
| 8 | #9 Avoid tool/message clones | Medium | Low |
| 9 | #10 Reduce lock acquisitions | Medium | Low |

---

## Notes

- Items #4 and #5 are coupled - fixing #4 (structured dispatch message) automatically fixes #5 (string parsing)
- Item #3 (async SQLite) may need careful consideration for durability guarantees
- Consider measuring before/after with tracing spans to validate impact
