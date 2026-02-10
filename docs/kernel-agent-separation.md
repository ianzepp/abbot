# Kernel / Agent Process Separation

## Motivation

The AI agent landscape changes weekly. Prompt strategies, tool-use patterns, and agent architectures are all moving targets. But the underlying infrastructure — process management, message passing, persistent storage, POSIX sockets — is stable.

The goal is to draw a hard boundary between the kernel (stable, long-lived) and agent logic (disposable, swappable). When the next wave of AI tooling arrives, you throw away the agents and keep the kernel.

## Kernel (abbotd)

The kernel owns:

- **Unix socket listener** — accepts connections from agent processes
- **Frame router** — routes frames between connected processes
- **Syscall dispatch** — EMS, exec, frames, file ops
- **Process supervisor** — spawn, kill, restart child processes
- **State persistence** — SQLite (Store, FrameStore, EMS)

The kernel does not know or care what agents do with LLMs. It doesn't assemble prompts, call inference APIs, or implement tool-use loops.

## Agent Processes

An agent is anything that can:

1. Connect to the kernel's Unix socket
2. Authenticate with a spawn-time token
3. Speak the frame protocol
4. Issue syscalls

Today that's Rust binaries (head, hand, mind, room). Tomorrow it could be Python, Node, or something that doesn't exist yet.

## Authentication

1. Socket bound with mode `0o600` (owner-only)
2. `SO_PEERCRED` check on accept — verify UID matches daemon
3. Spawn-time token — kernel generates a one-time secret, passes it to the child process (env var or arg), child sends it as its first frame. Kernel validates and drops unrecognized connections.

---

## Current Architecture (as-is)

Everything lives in a single `daemon` crate (~38k lines). The `Kernel` struct is a global singleton (`OnceLock<Arc<Kernel>>`) with 14 subsystem fields:

```
Kernel
  dispatcher:     RwLock<KernelDispatcher>
  external_tools: ExternalToolManager
  turns:          TurnRuntime
  sigcalls:       SigcallHub
  needs:          NeedKernel
  rooms:          RoomRegistry
  tick:           OnceLock<TickKernel>
  workspace:      PathBuf
  store:          OnceLock<Arc<Store>>
  activity_seq:   AtomicU64
  activity_last:  AtomicI64
  frames:         OnceLock<Arc<FrameStore>>
  ems:            OnceLock<EmsHandle>
  snapshot:       OnceLock<Arc<SnapshotManager>>
  session_locks:  SessionWriteLocks
```

Agent logic (bundles, prompt assembly, LLM calls, tool dispatch) and kernel infrastructure (frame routing, syscall dispatch, storage) are interleaved. Agents and kernel code both call `Kernel::get()` freely.

### Dependency graph

```
+--------------------------------------------------------------+
|                     KERNEL CRATE                              |
|  +--------------------------------------------------------+  |
|  | Dispatcher  (routes tool calls -> syscalls)             |  |
|  | FrameStore  (persists all frames)                       |  |
|  | RoomRegistry (spawns/manages room instances)            |  |
|  | SigcallHub  (broadcasts agent output)                   |  |
|  | TurnRuntime (manages external tool coordination)        |  |
|  | NeedKernel  (work queue)                                |  |
|  | TickKernel  (time-based events)                         |  |
|  | ExternalToolManager                                     |  |
|  +--------------------------------------------------------+  |
|                                                               |
|  Syscalls (dispatch.rs, fs/*.rs, chat/*.rs, room/*.rs, ...)  |
+--------------------------------------------------------------+
         ^         ^         ^         ^         ^
         |         |         |         |         |
    [HEAD]    [HAND]    [MIND]    [ROOM]   [SERVER]
```

Positive finding: the dependency flow is one-directional. Agents consume the kernel; the kernel never imports agent types. No circular dependencies exist today.

---

## Entanglement Points

Eight coupling points that make splitting difficult, ranked by severity.

### 1. Tool catalogs are hardcoded (HARDEST)

`head_catalog()`, `hand_catalog()`, `mind_catalog()`, `room_catalog()` are free functions in `syscalls/dispatch.rs`. Each agent type gets a fixed tool set compiled into the kernel. Adding a new agent type means editing dispatch.rs. Agents cannot define their own tool sets independently.

### 2. Global Kernel singleton (HARD)

Every syscall, every Door, every service calls `Kernel::get()`. Agents cannot be moved to separate crates without replacing this with dependency injection — passing a kernel handle (or a narrow trait) to agent factories instead.

### 3. Door pattern bidirectional coupling (MODERATE)

`WebSocketDoor` uses `Kernel::get()` in every method to dispatch back into the kernel. Room agents cannot run without a Door, and the Door cannot function without the kernel dispatcher. Every Door method follows this pattern:

```rust
async fn emit_chat_message(&self, content: &str) -> Result<(), String> {
    let Some(k) = Kernel::get() else { ... };
    let dispatcher = k.dispatcher().await;
    // ...dispatch Frame through kernel...
}
```

### 4. SnapshotManager shared state (MODERATE)

Kernel owns `SnapshotManager`. `HeadBundleBuilder` depends on it for rendering system prompts (tools, env, commandments). Agents cannot initialize without kernel-managed snapshots.

### 5. Store for conversation context (MODERATE)

All bundle builders need `Arc<Store>` for conversation history. Store queries happen during bundle building, before agents even start their LLM loop.

### 6. RoomRegistry vs Room type ownership (LOW)

`RoomRegistry` lives in `kernel/room_registry.rs`, but `Room` and `RoomAgent` live in `runtime/room/types.rs`. Splitting would force a decision about which crate owns the Room types.

### 7. Frame protocol & persistence (LOW)

All frames flow through the kernel dispatcher and are persisted to FrameStore. Agents emit frames via `SyscallContext` but never directly. This coupling is by design — frames *are* the kernel's protocol.

### 8. chat:llm straddles the boundary (LOW)

The `chat:llm` syscall (~600 lines) calls `LlmClient` and `llm_harness::chat_with_tools_retry()`. It is a kernel syscall but depends on agent-side LLM infrastructure (`hal/llm/`). In a separated architecture, LLM calling moves to the agent side and this syscall disappears entirely.

---

## Proposed Cut Line

### Stays in the kernel crate

| Directory | What |
|---|---|
| `kernel/` | Dispatcher, frame protocol, router, turns, needs, room registry, sigcall hub |
| `history/store.rs` | SQLite conversation persistence |
| `ems/` | Entity management service |
| `vfs/` | Virtual filesystem |
| `hal/fs`, `hal/net`, `hal/process`, `hal/git` | Non-LLM HAL modules |
| `syscalls/` | All syscall implementations (the stable API) |
| `server/` | HTTP/WebSocket endpoints |
| `runtime/` (subset) | app_config, config, preflight, trait_catalog, parser, snapshot, safe_mode, need, session_locks, process_state, system_bundle |
| `prompts/shared/`, `prompts/traits/`, `prompts/skills/` | Kernel-owned prompt fragments |

### Moves to agent crates

| Directory | What |
|---|---|
| `hal/llm/` | LlmClient, AnthropicClient, OpenAICompatClient, ToolSpec, Message types |
| `runtime/head/` | HeadBundleBuilder, HeadConfig, head execution |
| `runtime/hand/` | HandBundleBuilder, execute_hand_loop |
| `runtime/mind/` | MindLoop, MindLoopBundleBuilder |
| `runtime/room/` | RoomRunner, Room, RoomAgent, RoomType, Door implementations |
| `runtime/llm_harness.rs` | Retry orchestration |
| `prompts/head/`, `prompts/hand/`, `prompts/mind/`, `prompts/room/` | Agent-specific prompt templates |

### Syscall classification

**Kernel syscalls (pure infrastructure):** `chat:*`, `frames:*`, `session:*`, `tick:*`, `patch:*`, `fs:*`, `exec:*`, `net:*`

**Would move or disappear:** `chat:llm` (becomes agent-side LLM call, no longer a syscall)

**Domain syscalls (stay in kernel, consumed by agents):** `need:*`, `room:*`, `hand:run`, `docs:*`, `tool:*`, `traits:*`, `want:*`, `ems:*`

---

## Target Crate Structure

```
daemon/                    # kernel crate (abbotd binary)
  src/
    kernel/                # core infrastructure
    syscalls/              # dispatch + implementations
    history/
    ems/
    vfs/
    hal/                   # fs, net, process, git — no LLM
    prompts/shared/
    prompts/traits/
    server/
    runtime/               # config, preflight, snapshot, etc.

agent-head/                # interactive agent (new crate)
  src/
  prompts/head/

agent-room/                # multi-agent execution (new crate)
  src/
  prompts/room/

agent-hand/                # tool executor (optional)
agent-mind/                # reflection loop (optional)
```

The existing syscall interface is already the API contract — it just needs to become a wire protocol over the socket instead of an in-process trait call.

---

## Strategy Notes

1. **Tool catalogs move to agents.** Define a registration trait in the kernel. Agents declare their tool sets on connect, kernel validates against allowed syscalls.
2. **Dependency injection replaces the singleton.** Pass a kernel handle (or narrow trait object) to agent factories instead of `Kernel::get()`.
3. **Door becomes a socket client.** Instead of calling `Kernel::get()` internally, Door sends frames over the socket like any other client.
4. **Store and SnapshotManager stay kernel-side.** Agents receive initialized copies or query via syscalls.
5. **RoomRegistry stays in the kernel.** Room orchestration (spawning agents, managing rounds) is infrastructure, not agent logic.

---

## Monk OS Comparison: The Kernel Layer Stack

Monk OS (TypeScript/Bun, ~72k lines) proved the architectural design. Abbot (Rust, ~38k lines) proved the runtime. Comparing the two reveals what the kernel layer stack should be, and what abbot is missing.

### Layer-by-layer comparison

| Layer | Monk | Abbot | Gap? |
|-------|------|-------|------|
| **HAL** | 14 device types (storage, network, channel, redis) | 5 traits (fs, net, process, git, llm) | LLM moves to agents; otherwise covered |
| **Storage** | EMS + SQLite/Postgres dialect abstraction | Store + FrameStore + EMS (all SQLite) | Covered |
| **Observer pipeline** | 10-ring mutation pipeline (validate, enrich, persist, audit, notify) | Nothing | **Gap** |
| **VFS** | Model polymorphism (file, folder, device, proc, link) + mount table | Mount table + sandbox, no model dispatch | **Thin** |
| **Handle** | Unified I/O: `exec(msg) -> AsyncIterable<Response>` across file/socket/pipe/port/channel | Nothing — each subsystem has its own API | **Gap** |
| **Kernel core** | Process table, 60 modular kernel functions, signals | Dispatcher, Router, TurnRuntime, SigcallHub, Lanes | Both have it, different designs |
| **Dispatch** | Switch-based routing + sigcall registry (userspace extension) | Syscall trait + dispatch.rs + external tools | Covered |
| **Wire protocol** | MessagePack over Unix socket + gateway | Frame JSON over WebSocket | Covered |

### What monk does better

**Handle abstraction.** One trait, one `exec()` method, all I/O. Files, sockets, pipes, ports, channels — all implement the same interface. Reference-counted. Backpressure-aware. Inheritable by child processes. Abbot has no equivalent; each subsystem (VFS, HAL, EMS) exposes its own bespoke API.

**Observer pipeline.** Every EMS mutation flows through 10 rings:

```
Ring 0: Data Preparation    ─┐
Ring 1: Input Validation     │  Pre-database
Ring 2: Security             │  (can reject)
Ring 3: Business Logic       │
Ring 4: Enrichment          ─┘
Ring 5: Database            ─── SQL execution (persistence boundary)
Ring 6: Post-Database       ─┐
Ring 7: Audit                │  Post-database
Ring 8: Integration          │  (observe only)
Ring 9: Notification        ─┘
```

Validation, enrichment, audit, and notification are composed declaratively. Abbot's EMS does raw SQL with no pipeline.

**Model polymorphism in VFS.** Different path prefixes dispatch to different model implementations — `/proc/` to process table, `/dev/` to HAL, `/ems/` to entity store. Abbot's VFS just maps paths to host directories via a mount table.

**Sigcall registry.** Userspace processes register as handlers for custom syscall names, extending the kernel without recompilation. Abbot's external tool manager is close but not as clean.

**Modular kernel functions.** Monk extracted 60 small files from the kernel class for testability. Abbot's `Kernel` struct is a 14-field monolith with a global singleton accessor.

**Kernel/Dispatcher separation.** In monk, the Kernel and Dispatcher are peers. The OS class creates both and wires a callback between them. The Kernel doesn't know the Dispatcher exists — no import, no field, no reference. Syscall handlers are pure functions that receive explicit dependencies: `yield* fileOpen(proc, kernel, vfs, path, flags)`. In abbot, the Kernel owns the Dispatcher as a field and syscall handlers grab subsystems from the global singleton (`Kernel::get().unwrap().ems()`). See the dedicated section below.

### What abbot does better

**Frame protocol.** Richer than monk's Message/Response. Frames carry actor (authorship), trace (observability), parent correlation, deadline, and cancel semantics. Self-describing with optional field skipping. Monk's Message is just `{ op, data }`.

**Lane-based concurrency.** Immediate/Need/Room lanes prevent deadlocks by serializing mutations to shared state while allowing read-only syscalls to proceed freely. Monk is single-threaded JS — doesn't need this, but Rust does.

**TurnRuntime.** External tool rendezvous with per-turn cancellation, duplicate delivery prevention, and recent-completion tracking (max 256). Novel to agent coordination; nothing like it in monk.

**Backpressure in the dispatcher.** Watermark-based hysteresis with stall timeout. Monk has backpressure too (ping protocol at 1000-item high-water mark) but abbot's is more sophisticated.

**Real concurrency.** Rust async with actual multi-threaded parallelism. Monk's Bun Workers provide isolation but remain JS-bound.

### The two big gaps

The Handle abstraction and Observer pipeline are the primitives abbot needs to become a proper microkernel. Everything else — Frame protocol, Lanes, Turns, backpressure, SigcallHub — abbot already has, and in several cases does better than monk.

---

## Build vs. Rebuild: The Verdict

**Should we start from monk-os-kernel and rebuild from scratch in Rust?**

No.

### What a rewrite would cost

Re-implementing Frame protocol, Dispatcher, Router, Lane routing, FrameStore, Store, EMS, VFS mount table, HAL traits, SigcallHub, and TurnRuntime — all things that work today — just to arrive back at the current state minus agent code. That is ~7,000 lines of kernel infrastructure that is already correct and tested. Then you would still need to build Handle and Observer pipeline on top.

Some monk concepts (Bun Workers, `AsyncIterable`, single-threaded event loop) do not map 1:1 to Rust. A "port" is not the same problem in a language with real threads. The translation tax is real.

### What makes more sense

**Phase 1: Separate agents out.** As described above. This removes ~10,000 lines of entangled runtime code and eliminates 6 of the 8 entanglement points. The kernel becomes a clean ~28k-line crate.

**Phase 2a: Split the Kernel struct and kill the singleton.** Extract the 14-field god object into a Runtime coordinator + peer subsystems. Inject dependencies into syscall structs at registration time. Kill `Kernel::get()`. See the dedicated section below.

**Phase 2b: Port monk's missing abstractions into the now-clean kernel.**

1. **Handle trait** — unified I/O over file, socket, pipe, port, channel. One `async fn exec(&self, frame: Frame) -> FrameStream` method. Reference-counted. Capability-based access.

2. **Observer pipeline** — 10-ring mutation flow on EMS. Ring 1 accumulates validation errors. Ring 5 is the persistence boundary. Rings 7-9 are observe-only (audit, cache, notify). Observers are composable and declarative.

3. **VFS model polymorphism** — path prefixes dispatch to model handlers. `/ems/<kind>/` returns entity queries as YAML. All through the Handle interface.

4. **Sigcall registry** — userspace processes register as handlers for custom syscall names. Kernel routes unrecognized syscalls to the registered handler process via the frame protocol.

**Phase 3: Agent crates connect over the socket.** Agents are separate binaries that speak the frame protocol. They carry their own LLM clients, prompt templates, and tool-use loops. The kernel does not know they exist until they connect.

### The core insight

Monk proved the *design*. Abbot proved the *runtime*. A rewrite throws away the runtime to re-prove the design. It is cheaper to port the design into the existing runtime.

---

## Open Question: Is the Kernel Worth Building?

Every piece of the kernel exists off-the-shelf:

- **Frame routing** — NATS, ZeroMQ, Redis pub/sub, D-Bus
- **Process supervision** — systemd, launchd, supervisord
- **Persistent storage** — SQLite directly (agents connect to the DB)
- **Syscall dispatch** — gRPC, Cap'n Proto, JSON-RPC over sockets
- **Socket auth** — all of the above already handle this

If you replaced the kernel with NATS + systemd + raw SQLite + a thin spawner CLI, what would you actually lose?

The argument *for* building it: the value is in opinionated composition. Frames as the universal message format, syscalls as the capability model, EMS as the entity store — all wired together with a specific contract that agents rely on. Off-the-shelf tools give you primitives; you still write glue. And since no Rust-native message brokers exist at the same maturity as NATS/ZeroMQ (Go/C), the kernel is effectively a purpose-built "Rust-native NATS-for-one-machine" — a single binary with zero external dependencies.

The argument *against*: that's what every ESB project has ever said. The glue tends to grow until it becomes the thing you're trapped in. If the whole point is keeping a stable layer that outlives agent churn, maybe the kernel is also the wrong layer to stabilize. Maybe the real stable layer is just:

- The frame protocol spec (a document, not code)
- The SQLite schema
- A convention for how agents discover and connect to services

If that's true, the kernel should be as thin as possible — or not exist at all. The agents talk to NATS and SQLite directly, and the "kernel" is a README that describes the protocol.

The answer depends on whether the composition itself is the product, or just scaffolding.

---

## Handles Across Process Boundaries

Monk's Handles work in-process — the agent holds a trait object and calls `exec()` directly. In a separated architecture where agents are different OS processes connected by sockets, Handles work the same way file descriptors work in Unix: the Handle is a kernel-side object, and the agent gets an integer ID.

### The mechanism

**In-process (monk):**
```
agent calls handle.exec({ op: "read" })  →  trait method  →  response
```

**Cross-process (abbot target):**
```
agent sends Frame { name: "handle:exec", data: { hid: 42, op: "read" } }
    → socket →
kernel looks up handle 42 in process's handle table
    → calls exec() on the kernel-side Handle object
    → streams response Frames back over the socket
```

The agent never sees the Handle trait. It sees an integer (handle ID) and a uniform protocol for operating on it. The wire protocol *is* the Handle interface.

### Per-process handle table

The kernel maintains a handle table for each connected process:

```
Process A handle table:
  hid 0  →  stdin pipe       (kernel-side PipeHandle)
  hid 1  →  stdout pipe      (kernel-side PipeHandle)
  hid 2  →  stderr pipe      (kernel-side PipeHandle)
  hid 3  →  /data/config.json (kernel-side FileHandle)
  hid 4  →  ems query cursor  (kernel-side EmsHandle)
  hid 5  →  tcp:10.0.0.1:8080 (kernel-side SocketHandle)
```

When an agent opens something, the kernel creates the kernel-side Handle (which implements the trait), assigns an ID in that process's table, and returns the ID. When an agent disconnects, the kernel walks its handle table and decrements refcounts. Handles with zero refs get closed. Exactly like `close()` on process exit in Unix.

### Handle syscalls

```
handle:open   { path: "/data/foo.txt", flags: "r" }   → { hid: 3 }
handle:exec   { hid: 3, op: "read" }                  → streaming Frame responses
handle:exec   { hid: 3, op: "write", data: "..." }    → ok/error
handle:close  { hid: 3 }                              → done
handle:pass   { hid: 3, to_process: "uuid-..." }      → transfer ownership
```

`handle:exec` is the only interesting one. Everything else is lifecycle. The `op` inside the exec is what differentiates "read a file" from "query an EMS table" from "send bytes on a socket" — but the Frame structure is always the same.

### Handle passing between processes

Same as Unix `sendmsg()` with `SCM_RIGHTS`. Process A says "pass handle 3 to process B." The kernel looks up hid 3 in A's table, assigns a new hid in B's table pointing to the same kernel-side Handle, and increments the refcount. Optionally removes it from A's table (move vs. share semantics). The two processes now operate on the same underlying resource through different local IDs.

### Remote handles (over TCP)

Two options, both well-understood:

**Proxy (Plan 9 model).** The local kernel proxies handle operations to a remote kernel. The agent doesn't know or care that hid 5 points to a resource on another machine. The local kernel translates `handle:exec` into frames over TCP to the remote kernel, which owns the real Handle. This is Plan 9's `import`/`exportfs`.

**Direct connection.** The agent connects directly to the remote kernel's socket, authenticates, and gets handle IDs in the remote kernel's process table. No proxy. The agent just talks to a different kernel. Multiple kernel connections, each with their own handle namespace.

Proxy is more elegant (transparent to the agent). Direct is simpler to implement.

### Why this fits abbot's existing primitives

Frames are already the right shape. A Frame has `name` (operation), `data` (payload), `actor` (who), `parent_id` (correlation), and streaming response semantics (Ok/Item/Done/Error). That *is* the Handle protocol — you just need to add a handle ID to the routing.

The dispatcher already routes frames to handlers. SigcallHub already manages per-process delivery. FrameStore already persists everything. The addition is a handle table and a handful of syscalls — not a kernel rewrite.

The shift: instead of `name: "fs:read", data: { path: "/foo" }` (resolve path on every call), it becomes `name: "handle:exec", data: { hid: 3, op: "read" }` (path was resolved once at `handle:open` time). The kernel already knows what type of handle hid 3 is and dispatches accordingly.

### Why the Handle is the organizing abstraction

Monk feels *designed* because every module answers the same question: "what does this do to a Handle?" Abbot feels *iterated* because its subsystems (VFS, HAL, EMS, Store, FrameStore) each expose bespoke APIs that don't compose through a shared primitive.

Cleanup cycles don't fix that feeling. You can refactor names, flatten modules, delete dead code — but if the subsystems don't share a common abstraction, the codebase reads as a collection of parts rather than a design.

The Handle trait is the center of gravity. Once VFS, EMS, HAL, and process I/O all implement `exec(Frame) -> FrameStream`, every module becomes an answer to the same question. That is what makes a codebase feel designed — not the absence of iteration, but the presence of a single idea that everything else orbits.

---

## VFS Design: Files, Mounts, and EmsMount

### The monk VFS mistake

Monk's VFS tried to make files and database entities the same thing. Every file was an EMS entity with metadata columns, queryable by field. In theory you could `ls /data/` or `SELECT * FROM files WHERE status = 'active'` and get the same data. In practice it meant every file operation had SQL overhead, every EMS mutation had path-sync overhead, and the mental model was permanently split between "is this a file or a row?" It never became usable.

### VFS as just files — almost

The simplest VFS is a sandboxed mount table that resolves paths to files on disk. That's what abbot has today and it works. But there's one place where mount polymorphism earns its keep: **EMS data as files**.

LLM agents are extremely fluent at reading and writing files. That's what they're trained on — every coding agent reads source files, parses structured text, writes structured text back. Making them construct `ems:select { table: "tasks", where: { id: 1234 } }` with the right argument shape is harder than:

```
fs:read { path: "/ems/tasks/1234" }
```

Which returns:

```yaml
id: 1234
kind: task
status: active
priority: 2
prompt: "Fix the login bug"
data:
  assignee: head-01
  tags: [bug, auth]
```

To update, the agent writes back:

```yaml
status: done
```

No API to learn. Read a file, get YAML. Write YAML, update the entity. The agent already knows how to do this.

### EmsMount behavior

```
/ems/
  tasks/
    1234          → read: YAML of entity 1234
    1235          → write: merge YAML into entity 1235
  needs/
    5678
  memories/
    9012
```

- `fs:read { path: "/ems/tasks/1234" }` — VFS resolves through mount table, hits EmsMount, does `SELECT * FROM entities WHERE kind='task' AND id=1234`, returns YAML.
- `fs:write { path: "/ems/tasks/1234", data: "status: done\n" }` — EmsMount parses YAML, does `UPDATE entities SET ... WHERE id=1234`.
- `fs:read { path: "/ems/tasks/" }` — returns YAML list of task summaries (id + status + prompt first line).
- `fs:write { path: "/ems/tasks/", data: "prompt: Fix the login bug\n" }` — creates a new task, returns the path `/ems/tasks/1235`.

The VFS resolves the path through the mount table. If it lands on a FileMount, you get a file. If it lands on an EmsMount, you get an entity as YAML. The agent doesn't need to know which — it just reads and writes paths.

### Where file semantics work and where they don't

**Works great (the common case, ~80% of agent interactions):**

- Get entity by ID — read a path
- Update entity by ID — write to a path
- Create entity — write to the collection path
- Delete entity — unlink the path
- List entities in a kind — readdir

**Doesn't work:**

- Complex queries (`WHERE status = 'active' AND priority < 2 ORDER BY created_at DESC LIMIT 10`)
- Aggregations, joins, cross-kind queries

Simple CRUD goes through the mount. Complex queries stay as `ems:query` syscalls. They coexist because the mount is just another way to reach the same `entities` table.

### The mount table

```
/              → FileMount (sandbox root, ~/.abbot/sandbox/)
/tmp           → MemoryMount (ephemeral, in-memory)
/ems/tasks     → EmsMount { kind: "task" }
/ems/needs     → EmsMount { kind: "need" }
/ems/memories  → EmsMount { kind: "memory" }
/workspace     → FileMount (user's project directory)
```

Three mount types: FileMount, EmsMount, MemoryMount. No synthetic `/proc/`, no `/dev/`. Just files and entities — the two things agents actually interact with.

### Why YAML

Less syntactic noise than JSON (no braces, no quotes on keys, no commas). Easier for agents to write partial updates (just the fields you want to change). More natural for the "looks like a config file" mental model. JSON could be an option per-mount or per-request, but YAML as the default is friendlier for LLM consumers.

### Composing with Handles

The EmsMount doesn't conflict with the Handle abstraction — it composes with it:

```
handle:open { path: "/ems/tasks/1234" }        → EmsHandle (via EmsMount)
handle:open { path: "/workspace/src/main.rs" }  → FileHandle (via FileMount)
handle:exec { hid: 3, op: "read" }             → YAML or bytes, depending on handle type
```

Same interface, different backing. The VFS resolves the path to a mount, the mount produces a Handle, and the Handle does the work. The agent doesn't care what's behind the path.

---

## Splitting the Kernel Struct: Coordinator vs. Subsystems

### The problem

Abbot's `Kernel` struct plays two roles simultaneously:

1. **Coordinator** — creates subsystems, wires them together, manages boot lifecycle
2. **Service locator** — the global singleton that every syscall calls at runtime to find subsystems

Monk separates these cleanly. The `OS` class is the coordinator (creates Kernel, Dispatcher, VFS, EMS, HAL, wires callbacks). The `Kernel` class is just one subsystem — it owns processes and handles, nothing else. The `Dispatcher` is another peer subsystem. Neither owns the other.

```
Monk:
  OS (coordinator)
    ├── creates Kernel   (processes, handles, workers)
    ├── creates Dispatcher (routing, auth, backpressure)
    ├── creates VFS, EMS, HAL, Auth, LLM
    └── wires: kernel.onWorkerMessage = dispatcher.onWorkerMessage

Abbot today:
  Kernel (god object)
    ├── owns Dispatcher        (RwLock<KernelDispatcher> field)
    ├── owns Store, FrameStore, EMS, SigcallHub, TurnRuntime, ...
    └── exposes everything via Kernel::get() global singleton
```

### Why this matters

In monk, the Kernel doesn't know the Dispatcher exists. No import, no field, no reference. The dependency is strictly one-way: Dispatcher receives Kernel as a constructor parameter. When the OS wires them, it sets a callback:

```typescript
kernel.onWorkerMessage = (worker, msg) => dispatcher.onWorkerMessage(worker, msg);
```

Syscall handlers are pure functions with explicit dependencies:

```typescript
yield* fileOpen(proc, kernel, vfs, path, flags);
yield* emsQuery(proc, ems, model, filter);
```

Each handler receives exactly what it needs. Nothing global. Nothing implicit.

In abbot, the Dispatcher itself is actually clean — it's a stateless router with no kernel reference. But syscall handlers reach into the global to get what they need:

```rust
let k = Kernel::get().unwrap();
let ems = k.ems().unwrap();
let store = k.store().unwrap();
```

Every handler is implicitly coupled to the entire Kernel through the singleton. The dispatcher is decoupled, but the handlers aren't.

### The target structure

```
Abbas (coordinator — replaces the god-object Kernel)
  ├── creates Dispatcher       (routing, lanes, backpressure)
  ├── creates Frames, History, Entities
  ├── creates Vfs, Hal
  ├── creates Sigcalls, Turns, Needs
  └── wires them together, then gets out of the way
```

`Abbas` — Latin for "abbot, father" — is the coordinator that creates and wires all subsystems. After boot, it holds `Arc` references but is not called at runtime. It is not a singleton — it is the thing that ran `main()`.

The Dispatcher becomes a peer, not a child. Subsystems (Store, EMS, VFS, etc.) are peers, not fields on a god object.

### Dependency injection into syscalls

Two options for removing the `Kernel::get()` calls from syscall handlers:

**Option A: Fatten SyscallContext.** Context carries references to subsystems. Dispatcher builds the context with the right references before invoking the handler.

```rust
// Context carries everything
async fn execute(&self, ctx: &SyscallContext, data: Value, tx: Sender<Frame>) -> Result<()> {
    let ems = ctx.ems();
    // ...
}
```

Clean, but context grows to carry all subsystems even when a handler only needs one.

**Option B: Inject into the syscall struct (monk's pattern).** Each syscall struct holds `Arc` references to its dependencies, set at registration time. The `Syscall` trait stays the same.

```rust
struct EmsSelect {
    ems: EmsHandle,
}

#[async_trait]
impl Syscall for EmsSelect {
    async fn execute(&self, ctx: &SyscallContext, data: Value, tx: Sender<Frame>) -> Result<()> {
        let results = self.ems.select(...).await?;
        // ...
    }
}

struct FsRead {
    vfs: Arc<Vfs>,
}

#[async_trait]
impl Syscall for FsRead {
    async fn execute(&self, ctx: &SyscallContext, data: Value, tx: Sender<Frame>) -> Result<()> {
        let content = self.vfs.read(...).await?;
        // ...
    }
}
```

Registration wires the dependencies:

```rust
fn register_all(dispatcher: &mut Dispatcher, vfs: Arc<Vfs>, ems: EmsHandle, ...) {
    dispatcher.register(Arc::new(EmsSelect { ems: ems.clone() }));
    dispatcher.register(Arc::new(FsRead { vfs: vfs.clone() }));
    // ...
}
```

Option B is cleaner because different syscalls need different subsystems. `fs:read` has no reason to have access to EMS. Each handler gets exactly what it needs and nothing more — the principle of least authority applied to syscall implementations.

### What `Kernel::get()` becomes

It doesn't. The global singleton is deleted. Every code path that currently calls `Kernel::get()` either:

1. **Is a syscall handler** — gets its dependencies via struct fields (Option B above)
2. **Is a server/WebSocket handler** — receives the Dispatcher as a parameter from Abbas at server startup
3. **Is an agent** — talks to the kernel over a socket, no in-process access at all

No code needs to reach into a global to find a subsystem. Dependencies are explicit, testable, and visible in the type signature.

---

## Naming Conventions

### The problem today

| Current name | Role | Suffix |
|---|---|---|
| `KernelDispatcher` | Routes frames to syscalls | (none) |
| `ExternalToolManager` | Tool registry | Manager |
| `SnapshotManager` | Config snapshots | Manager |
| `TurnRuntime` | External tool rendezvous | Runtime |
| `SigcallHub` | Frame delivery | Hub |
| `NeedKernel` | Work queue | Kernel (!!) |
| `TickKernel` | Timer events | Kernel (!!) |
| `RoomRegistry` | Room tracking | Registry |
| `Store` | History persistence | (none) |
| `FrameStore` | Frame persistence | Store |
| `EmsService` | Entity CRUD | Service |
| `EmsHandle` | `Arc<Mutex<EmsService>>` | Handle (!!) |

Three collisions: `NeedKernel`/`TickKernel` sound like they ARE kernels. `EmsHandle` will collide with the Handle abstraction. And five different suffixes (Manager, Runtime, Hub, Registry, Service) for things that all mean "a subsystem that coordinates X."

### The scheme

**Data types:** no suffix. `Frame`, `Mount`, `Process`, `Room`.

**Persistent storage:** `Store` suffix. `FrameStore`, `HistoryStore`, `EntityStore`.

**Request-scoped state:** `Context` suffix. `SyscallContext`.

**Subsystems:** plural nouns. They're collections of the thing they manage.

| Current | New | Rationale |
|---|---|---|
| `Kernel` | `Abbas` | Latin for "abbot" — the coordinator/overseer |
| `KernelDispatcher` | `Dispatcher` | Drop the prefix, it's a peer now |
| `NeedKernel` | `Needs` | Plural noun, manages the need queue |
| `TickKernel` | `Ticks` | Plural noun, manages tick events |
| `TurnRuntime` | `Turns` | Plural noun, manages turn lifecycle |
| `SigcallHub` | `Sigcalls` | Plural noun, manages sigcall delivery |
| `ExternalToolManager` | `ExternalTools` | Plural noun |
| `SnapshotManager` | `Snapshots` | Plural noun |
| `RoomRegistry` | `Rooms` | Plural noun |
| `EmsService` | `Entities` | Plural noun, what it actually manages |
| `EmsHandle` | `Arc<Entities>` | No wrapper type, just the Arc |
| `Store` | `HistoryStore` | Disambiguate from other stores |
| `KernelRouter` | `Router` | Drop the prefix |

The pattern: `Abbas` creates `Dispatcher`, `Needs`, `Turns`, `Sigcalls`, `Rooms`, `Entities`, `FrameStore`, `HistoryStore`, `Snapshots`, `ExternalTools`. All plural nouns except stores and the dispatcher. No suffix taxonomy to memorize.

### Why `Abbas` and not more Latin

The faber project (and its self-hosting compiler rivus) proved that comprehensive Latin naming works — but only when the *entire* language is Latin. Keywords, types, methods, variables, operators — all Latin, all following real grammatical conjugation patterns. The immersion is what makes it readable.

In a Rust codebase, Latin type names would be islands in an English sea: `struct Ansa` next to `Arc<AtomicU64>`, `async_trait`, `Result`, `impl Send + Sync`. No immersion, just cognitive overhead. `Ansa` doesn't encode anything that `Handle` doesn't — it's a synonym, not a semantic upgrade.

`Abbas` works as the single exception because it's a proper noun — the project's name made concrete. Same way Kubernetes is Greek for "helmsman" without expecting all K8s code to be in Greek. Everything else stays English, stays readable, stays conventional Rust.

```rust
let abbas = Abbas::boot(config).await?;
// abbas.dispatcher, abbas.entities, abbas.frames, ...
```

A little touch of class. Nothing more.

---

## Architectural Purity Principles

Derived from monk-os-kernel's design and applied to abbot's Rust rewrite. These are the invariants the kernel must maintain.

### 1. Message-pure internals

Structured objects flow between components. Serialization happens only at true I/O boundaries — disk and network. Nothing else.

**Today's impurity:** Abbot's `Frame.data` is `serde_json::Value` everywhere, even for in-process communication. A syscall constructs a `Value`, the dispatcher passes a `Value`, the handler parses the `Value`. Three serialization-adjacent operations for a call that never left the process.

**Target:** Frame payloads are typed Rust structs internally. `serde_json::Value` exists only at two boundaries: (1) writing to SQLite/FrameStore, (2) sending over WebSocket/Unix socket. Inside the kernel, a `Frame<FsReadArgs>` carries typed data. The generic `Frame<Value>` is the wire format, not the internal format.

### 2. Syscalls are streaming by default

Each syscall is an async stream that yields `Frame` responses independently. No buffering to arrays. No collecting results before sending.

- **Terminal frames** (`Done`, `Error`) signal stream end
- **Non-terminal frames** (`Ok`, `Item`, `Event`, `Progress`) yield indefinitely
- A syscall that returns one result yields `Ok` then `Done`
- A syscall that returns many results yields `Item` per result then `Done`
- A syscall that watches for changes yields `Event` indefinitely until cancelled

**Today's impurity:** Some syscall implementations collect results into a `Vec<Value>`, serialize the whole thing, and send it as a single `Ok` frame. This defeats backpressure and prevents progressive rendering.

**Target:** Every syscall implementation is an async stream. The dispatcher wraps each stream with backpressure control. The handler never sees the transport — it yields frames and the dispatcher manages delivery.

### 3. Processes have identity and handle tables

Each process (agent or service) has:

- **UUID identity** — not integer PIDs, not actor strings
- **Handle table** — file descriptors (integers) map to kernel-side Handle UUIDs
- **Standard fds** — `0` = recv (frames in), `1` = send (frames out), `2` = warn (diagnostics)
- **State machine** — `starting → running → stopped → zombie`

**Today's impurity:** Agents are in-process async tasks identified by actor strings (`"head/abc123"`). There is no process table, no handle table, no per-process fd mapping. Agent identity is a convention, not a kernel-enforced abstraction.

**Target:** `Abbas` maintains a process table. When an agent connects over the Unix socket, the kernel creates a `Process` entry with a UUID, an empty handle table, and standard fds wired to the socket. The process is the unit of identity, isolation, and cleanup.

### 4. Handles are capabilities

If you have the handle, you have permission. Access is granted at `open()` time — the kernel checks permissions once and issues a handle. After that, the handle itself is the proof of authorization. No rwx bits. No per-operation permission checks.

**Today's impurity:** Abbot checks `ctx.can_mutate()` on every mutating syscall, using the actor string to determine role. This is RBAC bolted onto syscalls. It's also fragile — the actor string is trusted, not verified.

**Target:** When a process calls `handle:open`, the kernel checks whether that process is allowed to open that resource with those flags. If yes, it returns a handle with the granted capabilities baked in. Subsequent `handle:exec` calls on that handle don't re-check — the handle *is* the permission. Revoking access means closing the handle.

### 5. Multiplexed streaming per process

A single process can have multiple concurrent syscalls in flight, each identified by a stream ID (the request Frame's `id`). The dispatcher routes response frames back to the correct stream via the process's send fd.

**Today's impurity:** Each `dispatch()` call creates a dedicated `mpsc` channel. There's no multiplexing — each call gets its own transport. This works for in-process agents but doesn't translate to socket-based agents where a single connection carries multiple concurrent streams.

**Target:** The Unix socket connection to each process carries multiplexed frames. Outbound frames include the `parent_id` (correlation to the request). The client-side SDK demuxes responses by `parent_id` into per-stream receivers. This is already how the WebSocket protocol works in abbot — it just needs to become the universal model.

### 6. Message pipes, not byte streams

Inter-process pipes carry structured `Frame` objects, not byte streams. When process A sends a frame to process B through a pipe, there is no serialization — the same Rust struct arrives on the other side.

**Today's impurity:** There are no inter-process pipes at all. Agents communicate indirectly through the kernel (emit a frame, kernel broadcasts it, another agent picks it up). Direct process-to-process communication doesn't exist.

**Target:** A pipe is a pair of Handles — one write end, one read end. The kernel creates both, puts the write end in process A's handle table and the read end in process B's handle table. Frames written to the pipe appear in the reader's stream. For in-process agents (same OS process), this is zero-copy. For socket-based agents, the kernel serializes at the boundary — but the abstraction is the same.

### 7. Protocol channels hide wire formats

HTTP, WebSocket, and other protocols are abstracted behind a unified channel interface. Agents never see wire protocols — just `send(frame)` and `recv() -> Frame`. The kernel handles framing, encoding, and connection lifecycle.

**Today's impurity:** The WebSocket handler in `server/websocket.rs` manually parses inbound messages, constructs frames, dispatches them, collects responses, and serializes them back. The wire protocol is visible throughout the server layer.

**Target:** A WebSocket connection is a channel Handle. The kernel's channel implementation handles the WebSocket framing. The agent on the other end of the Handle sees only Frames. Adding a new wire protocol (TCP, Unix socket, HTTP/2) means implementing a new channel type, not modifying the dispatcher or any syscall.

### Summary

| Principle | Today | Target |
|---|---|---|
| Message-pure internals | `serde_json::Value` everywhere | Typed structs; JSON only at I/O boundaries |
| Streaming syscalls | Some buffer to arrays | All syscalls are async streams |
| Process identity | Actor strings, no process table | UUID processes with handle tables |
| Capability security | RBAC via actor string per call | Handle = permission, checked once at open |
| Multiplexed streams | Dedicated channel per dispatch | Multiplexed frames over single connection |
| Message pipes | No inter-process pipes | Structured Frame pipes, zero-copy in-process |
| Protocol channels | Wire format visible in server | Channel Handles hide wire protocol |
