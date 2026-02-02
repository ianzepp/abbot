# Vision Phase 3: Policy, UX, and Extensibility

Phase 3 builds on Phase 2's hard boundary (caps + VFS v1) to make the system:
- easier to use (less manual configuration)
- safer by default (better policy, better auditing)
- extensible (services/plugins as userland)

The key constraint from Phase 2 still applies: do not introduce complexity until it has a clear, recurring use case.

## 1) Capability Policy Without LLM Reliance

Phase 3 assumes the LLM is not reliable at requesting caps.

Policy remains kernel-driven:
- The kernel assigns a baseline cap set per scope and per syscall name.
- The kernel denies or clamps requests outside that set.
- Optional escalation is mediated by the human (CLI/UI), not by the LLM.

Escalation triggers (examples):
- attempt to access an unmapped VFS prefix
- attempt to fetch a non-allowlisted host
- attempt to run a non-allowlisted program

Escalation UX (minimal):
- show the denied request and the smallest grant that would allow it
- provide a one-shot approval option (session/task scoped)
- provide a persistent approval option (writes to mounts/policy config)

## 2) VFS v2 (Only If Needed)

VFS v1 is just mounts + ro/rw + safe resolution.
VFS v2 adds narrowly-scoped knobs only when real pain appears.

Candidate additions (opt-in):
- path allow/deny patterns inside a mount
- read-only subpaths within an rw mount
- symlink handling policy (follow/deny)
- max file size / max read bytes per call

Rule: each addition must come with a concrete incident it prevents or an operational need it enables.

## 3) Better Streaming UX (Not Just Bounded Capture)

Phase 1 and 2 can work with bounded stdout/stderr capture.
Phase 3 adds true streaming for long-running operations.

Targets:
- `proc.run@N`: stream stdout/stderr as `item` frames while process runs
- `net.fetch@N`: stream download progress and optionally `data` chunks

Requirements:
- backpressure enforcement stays in the kernel
- cancellation kills the underlying operation promptly

## 4) Audit Trail in EMS

Persist syscall traces in EMS to support:
- debugging and replay (where possible)
- UI rendering of progress and outcomes
- policy tuning based on real usage

Minimum trace fields:
- `id`, `parent_id`, `scope`, `name`
- start/end timestamps
- termination (`ok`/`error`, error code)
- counters: emitted items, emitted bytes
- resources: touched VFS paths, spawned processes, net hosts

Phase 3 should define a stable schema for these trace entities.

## 5) Sigcalls / Userland Services

Introduce a registry that allows non-kernel services to handle operations by name.

Concept:
- kernel routes `syscall:req name=foo.bar@1` to a registered handler
- handler emits the same `op` frames (`item`, `progress`, `ok`, `error`)
- kernel still enforces caps and accounting at the boundary

Use cases:
- code indexing service
- repo watcher service
- diagnostics/lint service

Implementation note:
- services can start in-process (Rust modules) and later move out-of-process without changing the protocol.

## 6) Policy as Data (If/When Needed)

If Phase 2's static config becomes insufficient, Phase 3 can introduce a small policy config.
Avoid a full policy language.

Policy config candidates:
- program allowlist for `proc.run`
- host allowlist for `net.fetch`
- per-scope default caps
- per-scope max time/bytes

If this grows beyond a page, split into separate files:
- `<sandbox>/mounts.toml`
- `<sandbox>/policy.toml`

## Acceptance Criteria

Phase 3 is complete when:
- humans can approve/deny escalation without editing code
- long-running operations stream progress and can be cancelled cleanly
- every syscall run can be inspected post-hoc via EMS
- new tools can be implemented as userland services without kernel changes
