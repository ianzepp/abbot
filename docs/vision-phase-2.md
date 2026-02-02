# Vision Phase 2: Caps + VFS v1 (Minimal)

Phase 2 turns the Phase 1 kernel protocol into a usable safety boundary.

The guiding constraint for Phase 2 is: do not build a policy language.
Implement the smallest set of primitives that unlock real sandboxing and keep Abbot usable.

Deliverables:
- capability checks on syscall handlers (least privilege)
- VFS v1 mounts for path sandboxing and explicit host directory mapping

Non-goals (defer):
- pattern allow/deny lists
- per-mount file size limits
- symlink policy knobs
- per-task ephemeral mounts
- user approval UX

## Capabilities (Caps)

Caps are explicit grants attached to a kernel request.

Phase 2 cap shape:
- strings (simple to serialize and log)
- interpreted by kernel policy + enforced by syscall handlers

Cap examples:
- `cap.fs.read:/workspace/`
- `cap.fs.write:/workspace/`
- `cap.fs.read:/mnt/foo/`
- `cap.proc.run:git`
- `cap.net.fetch:api.github.com`

Rules:
- deny by default
- allow small safe defaults for core workflows (e.g. `cap.fs.read:/workspace/`)
- syscall handlers must validate caps; tools cannot bypass

Who defines caps:
- callers request caps
- the kernel grants/denies via policy
- handlers enforce

Phase 2 policy is intentionally simple:
- small built-in allowlist based on scope (`task/<id>` gets workspace read)
- optional static config overrides

## VFS v1

VFS v1 is only a mount table plus safe path resolution.

### VFS Paths

- All filesystem syscalls accept VFS paths only.
- VFS paths are absolute and rooted at a mount prefix.

Examples:
- `/workspace/src/main.rs`
- `/tmp/abbot/log.txt`

### Mount Table

A mount maps a VFS prefix to a host directory.

Each mount has:
- `prefix`: VFS prefix (e.g. `/workspace`)
- `host`: absolute host path
- `mode`: `ro` or `rw`

No other knobs in v1.

### Resolution Rules

`resolve(vfs_path)` performs:
1) validate absolute VFS path
2) find the longest matching mount prefix
3) compute `rel = vfs_path.strip_prefix(prefix)`
4) join `host + rel` and normalize
5) canonicalize and verify the resolved host path stays under the mount host root

Reject cases:
- no mount matches
- `..` traversal
- canonical path escapes mount root

### Write Checks

- Writes require the selected mount to be `rw`.
- Reads are allowed for both `ro` and `rw`.

### Default Mounts

Phase 2 ships with exactly two mounts:
- `/workspace` -> repo root (rw)
- `/tmp` -> system temp dir (rw)

Additional mounts are optional.

## Config

If mounts remain simple, keep them in `<sandbox>/config.toml`.
If mounts grow beyond a small handful, split them into `<sandbox>/mounts.toml`.

Phase 2 recommended config (simple):

```toml
[mounts.workspace]
host = "/abs/path/to/repo"
mode = "rw"
prefix = "/workspace"

[mounts.tmp]
host = "/tmp"
mode = "rw"
prefix = "/tmp"
```

## Integration Plan

Order of work:
1) Introduce a `vfs` module with `MountTable` + `resolve()`.
2) Update fs syscall handlers to take VFS paths and use VFS resolution.
3) Introduce cap checks for fs operations:
   - require `cap.fs.read:<prefix>` for reads
   - require `cap.fs.write:<prefix>` for writes
4) Route existing file tools through the fs syscall handlers.
5) Add auditing fields (at least: selected mount, vfs path, resolved host path).

Acceptance criteria:
- A tool cannot read or write outside `/workspace` and `/tmp`.
- Adding a new external directory requires an explicit mount entry.
- No complex policy knobs are required to use the system.
