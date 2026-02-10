# Monk OS vs Abbot: Kernel Comparison

This document is a running comparison between Monk OS (TypeScript/Bun) and Abbot (Rust).

It was extracted from `docs/kernel-agent-separation.md` so it can be maintained as Abbot evolves.

## The Kernel Layer Stack

Monk OS (TypeScript/Bun, ~72k lines) proved the architectural design. Abbot (Rust, ~38k lines) proved the runtime. Comparing the two reveals what the kernel layer stack should be, and what Abbot is missing.

### Layer-by-layer comparison

| Layer | Monk | Abbot | Gap? |
|-------|------|-------|------|
| **HAL** | 14 device types (storage, network, channel, redis) | 4 traits (fs, net, process, git) + an LLM client module | LLM moves to agents; otherwise covered |
| **Storage** | EMS + SQLite/Postgres dialect abstraction | Store + FrameStore + EMS (all SQLite) | Covered |
| **Observer pipeline** | 10-ring mutation pipeline (validate, enrich, persist, audit, notify) | Nothing | **Gap** |
| **VFS** | Model polymorphism (file, folder, device, proc, link) + mount table | Mount table + sandbox, no model dispatch | **Thin** |
| **Handle** | Unified I/O: `exec(msg) -> AsyncIterable<Response>` across file/socket/pipe/port/channel | Nothing -- each subsystem has its own API | **Gap** |
| **Kernel core** | Process table, modular kernel functions, signals | Dispatcher, Router, TurnRuntime, SigcallHub, Lanes | Both have it, different designs |
| **Dispatch** | Switch-based routing + sigcall registry (userspace extension) | Syscall trait + dispatch.rs + external tools | Covered |
| **Wire protocol** | MessagePack over TCP + WebSocket via Gateway | Frame JSON over WebSocket | Covered |

### What monk does better

**Handle abstraction.** One trait, one `exec()` method, all I/O. Files, sockets, pipes, ports, channels all implement the same interface. Reference-counted. Backpressure-aware. Inheritable by child processes. Abbot has no equivalent; each subsystem (VFS, HAL, EMS) exposes its own bespoke API.

**Observer pipeline.** Every EMS mutation flows through 10 rings:

```
Ring 0: Data Preparation    +\
Ring 1: Input Validation    |  Pre-database
Ring 2: Security            |  (can reject)
Ring 3: Business Logic      |
Ring 4: Enrichment          +/
Ring 5: Database            --- SQL execution (persistence boundary)
Ring 6: Post-Database       +\
Ring 7: Audit               |  Post-database
Ring 8: Integration         |  (observe only)
Ring 9: Notification        +/
```

Validation, enrichment, audit, and notification are composed declaratively. Abbot's EMS does raw SQL with no pipeline.

**Model polymorphism in VFS.** Different path prefixes dispatch to different model implementations -- `/proc/` to process table, `/dev/` to HAL, `/ems/` to entity store. Abbot's VFS just maps paths to host directories via a mount table.

**Sigcall registry.** Userspace processes register as handlers for custom syscall names, extending the kernel without recompilation. Abbot's external tool manager is close but not as clean.

**Modular kernel functions.** Monk extracted 60 small files from the kernel class for testability. Abbot's `Kernel` struct is a 14-field monolith with a global singleton accessor.

**Kernel/Dispatcher separation.** In monk, the Kernel and Dispatcher are peers. The OS class creates both and wires a callback between them. The Kernel doesn't know the Dispatcher exists -- no import, no field, no reference. Syscall handlers are pure functions that receive explicit dependencies: `yield* fileOpen(proc, kernel, vfs, path, flags)`. In abbot, the Kernel owns the Dispatcher as a field and syscall handlers grab subsystems from the global singleton (`Kernel::get().unwrap().ems()`). See the dedicated section in `docs/kernel-agent-separation.md`.

### What abbot does better

**Frame protocol.** Richer than monk's Message/Response. Frames carry actor (authorship), trace (observability), parent correlation, deadline, and cancel semantics. Self-describing with optional field skipping. Monk's Message is just `{ op, data }`.

**Lane-based concurrency.** Immediate/Need/Room lanes prevent deadlocks by serializing mutations to shared state while allowing read-only syscalls to proceed freely. Monk is single-threaded JS -- doesn't need this, but Rust does.

**TurnRuntime.** External tool rendezvous with per-turn cancellation, duplicate delivery prevention, and recent-completion tracking (max 256). Novel to agent coordination; nothing like it in monk.

**Backpressure in the dispatcher.** Watermark-based hysteresis with stall timeout. Monk has backpressure too (ping protocol at 1000-item high-water mark) but Abbot's is more sophisticated.

**Real concurrency.** Rust async with actual multi-threaded parallelism. Monk's Bun Workers provide isolation but remain JS-bound.

### The two big gaps

The Handle abstraction and Observer pipeline are the primitives Abbot needs to become a proper microkernel. Everything else -- Frame protocol, Lanes, Turns, backpressure, SigcallHub -- Abbot already has, and in several cases does better than monk.
