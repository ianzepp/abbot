# Autopoiesis: A Self-Authoring Operating System

## Vision

Three projects — **monk-os-kernel**, **abbot**, and **faber** — converge into a single
system where the OS kernel, the userspace language, and the agents living inside the
system form a closed, self-sustaining loop.

The agents write the programs. The language was designed for those agents. The filesystem
compiles it. The kernel runs it. The system authors itself.

```
┌─────────────────────────────────────────────────────┐
│  Faber programs (userspace)                         │
│  Written by humans or by LLM agents                 │
│  Latin syntax, morphological methods                │
├─────────────────────────────────────────────────────┤
│  Faber compiler (runtime layer)                     │
│  Compiles to target: TS for scripts,                │
│  Rust for kernel extensions, Go for services        │
├─────────────────────────────────────────────────────┤
│  abbot kernel (Rust)                                │
│  Frame protocol, syscall dispatcher,                │
│  VFS, EMS, HAL, process/room management             │
└─────────────────────────────────────────────────────┘
```

## Why these three fit together

### monk-os-kernel: the blueprint

Monk established the architectural DNA:

- **Everything is a file** — unified namespace (Plan 9)
- **Everything is a handle** — unified I/O via `exec(handle, msg) → AsyncIterable<Response>`
- **Message-driven kernel** — no shared state, only messages
- **8-ring observer pipeline** — validate, transform, persist, cache, notify
- **HAL with 17 device interfaces** — abstract all hardware behind traits
- **Streaming with backpressure** — async generators as the default API
- **Capability-based security** — handles are permissions

Monk was written in TypeScript on Bun. It proved the design. It was never meant to be
the final runtime.

### abbot: the kernel in Rust

Abbot already carries monk's DNA:

| Monk concept              | Abbot equivalent                          | Status        |
|---------------------------|-------------------------------------------|---------------|
| Message / Response        | Frame (Req/Ok/Item/Done/Error)            | Evolved       |
| Syscall dispatcher        | KernelDispatcher (trait-based + lanes)     | Evolved       |
| VFS                       | vfs/ (mount table, sandbox, resolution)    | Present, thin |
| EMS                       | ems/ (unified entity table, CRUD)          | Present, thin |
| HAL                       | hal/ (fs, git, llm, net, process)          | Present       |
| Streaming + backpressure  | Watermark-based flow control               | Present       |
| Handle abstraction        | —                                          | Gap           |
| Observer pipeline         | —                                          | Gap           |
| Model polymorphism        | —                                          | Gap           |

The path is not "rewrite abbot as monk-in-Rust." It is "finish what abbot already
started" — deepen VFS, EMS, and the handle model to match monk's design.

### faber: the userspace language

Faber is an LLM-optimized intermediate representation that compiles to multiple target
languages. It was designed so that LLMs can write it reliably (96-98% accuracy proven
across 17 models in faber-trials) and humans can review it easily.

Three properties make it the natural userspace language for this stack:

**1. Morphological methods encode the Handle interface.**

Monk's handles respond to messages with sync/async/streaming semantics. Faber's Latin
verb conjugation system encodes exactly those semantics in method names:

```fab
handle.lege()           # read (imperative, sync, mutating)
handle.lecta()          # read (perfect, sync, allocating — returns copy)
handle.legebit()        # read (future indicative, async, mutating)
handle.legens()         # read (present participle — streaming generator)
```

One stem, four I/O modes. The morphology IS the syscall dispatch hint.

**2. `@ verte` annotations are a syscall bridge.**

Faber already supports per-target translation annotations. A new `abbot` target maps
Latin verbs directly to kernel syscalls:

```fab
@ verte abbot "fs:write"
functio scribe(textus via, textus data) -> vacuum { ... }

@ verte abbot "ems:select"
functio quaere(textus genus, tabula filtrum) -> lista<tabula> { ... }
```

Faber programs don't call syscalls. They call Latin verbs that compile to syscalls.
The language IS the SDK.

**3. LLM agents can write Faber programs.**

faber-trials proved this across 17 models. The agents living inside abbot can generate
Faber source, the compiler can emit target code, and the kernel can run it — closing
the autopoietic loop.

## The compilation pipeline lives in VFS

Monk's VFS already had a module loader that transpiled TypeScript on demand. The same
pattern applies to Faber. A compiler mount driver makes compilation transparent:

```
/app/salve.fab    → source (Faber)
/app/salve.ts     → compiled on read (TypeScript target)
/app/salve.rs     → compiled on read (Rust target)
```

The filesystem IS the build system. Read the source, get Faber. Read the compiled
output, get the target. The VFS manages the compilation cache.

## The autopoietic loop

Traditional operating systems: humans write programs, the OS runs them.

This system:

```
User: "I need a service that watches /data and indexes new files"

    → Head agent interprets the request as a Need
    → Hand agent writes a Faber program:

        incipiet {
            fixum watcher = cede aperi("/data", "vigila")
            dum verum {
                fixum event = cede watcher.legens()
                cede indicem_adde(event.via)
            }
        }

    → Faber compiler emits target code
    → Kernel spawns it as a process with VFS handles
    → Process runs, watching /data through the kernel
```

The OS just wrote its own daemon. In a language proven to be LLM-writable. Running on
a kernel that already understands the message protocol. That is not an application
platform — it is a system that authors itself.

## Implementation roadmap

### Phase 1: Deepen monk DNA in abbot

Finish the architectural inheritance. These are internal kernel improvements that do not
depend on Faber:

- **Handle abstraction** — unify file, socket, pipe, port, channel under one trait
  with ref-counting and capability-based access
- **EMS observer pipeline** — implement monk's 8-ring pattern
  (validate → transform → persist → cache → notify)
- **VFS model polymorphism** — different path prefixes dispatch to different model
  implementations (`/proc/`, `/dev/`, `/ems/`)

### Phase 2: Faber as abbot target

Build the bridge between Faber and the abbot kernel:

- **`@ verte abbot` target** — new codegen backend that emits syscall-compatible code
- **Faber syscall stdlib** — Latin-named wrappers for `fs:*`, `ems:*`, `handle:*`,
  `room:*`, `need:*` syscalls
- **VFS compiler mount** — mount driver that compiles `.fab` files on read, caching
  the output
- **Process bootstrap** — kernel can spawn a compiled Faber program as a room/process

### Phase 3: Close the loop

Enable the agents to write and deploy Faber programs autonomously:

- **Hand tool: `fab:compile`** — agent can compile Faber source to target
- **Hand tool: `fab:deploy`** — agent can deploy a compiled program as a kernel process
- **Mind integration** — mind loop can observe running Faber processes, propose
  improvements, write patches in Faber
- **Hot-swap** — replace a running Faber process without kernel restart

### Phase 4: Self-hosting ambitions

Longer-term possibilities once the loop is closed:

- **Faber→Rust codegen maturity** — write kernel extensions in Faber
- **Agent-written observers** — EMS pipeline rings defined in Faber, compiled and
  loaded at runtime
- **Agent-written device drivers** — HAL implementations authored by agents
- **Faber-in-Faber compilation** — rivus (self-hosting compiler) running as a kernel
  process, compiling new Faber programs inside the OS

## The rule being broken

Traditional OS design assumes a human programmer external to the system. The kernel
provides primitives. The programmer uses them. The two never meet.

Here, the programmer lives inside the system. The language was designed for that
programmer. The compiler runs inside the filesystem. The kernel runs the output. The
programmer observes the result and writes the next program.

That is autopoiesis — a system that produces the components which produce the system.
