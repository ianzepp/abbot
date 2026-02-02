# Vision Phase 2: Mutation Control + VFS v1

Phase 2 turns the Phase 1 kernel protocol into a usable safety boundary.

The guiding constraint for Phase 2 is: do not build a policy language.
Implement the smallest set of primitives that unlock real sandboxing and keep Abbot usable.

Deliverables:
- mutation control via caller identity (scope)
- VFS v1 with HostMount for path sandboxing

Non-goals (defer):
- pattern allow/deny lists
- per-mount file size limits
- symlink policy knobs
- per-task ephemeral mounts
- user approval UX
- non-host mount types (EntityMount, ProcMount, etc.)

## Mutation Control

Mutation permission is derived from the caller's scope, which is already present on every syscall request.

Scope format: `<type>/<id>` (e.g., `head/4`, `hand/22`)

Rules:
- `head/*` callers may execute mutating syscalls (fs:write, proc:run, etc.)
- `hand/*` callers are restricted to read-only syscalls (fs:read, fs:list, etc.)
- deny by default for unknown scope prefixes

Syscall handlers check mutation permission:
```rust
fn check_mutation_allowed(scope: &str) -> Result<(), KernelError> {
    if scope.starts_with("head/") {
        Ok(())
    } else {
        Err(KernelError::forbidden("mutation requires head scope"))
    }
}
```

This replaces the capability model. No cap strings, no request/grant flow.
The caller declares identity via scope; the kernel enforces mutation rules.

## VFS v1

VFS v1 provides path sandboxing via a mount table with HostMount backends.

### Architecture

VFS is a standalone module that:
- owns the mount table
- handles path resolution
- is initialized at kernel boot
- reads config from `config.toml`

The `fs:*` syscalls delegate to VFS for all path operations.

```
fs:read syscall
    └──> VFS module
           ├── mount table
           └── resolve(path) -> host path
                 └── HostMount operations
```

### Mount Table

A mount maps a VFS prefix to a host directory.

Each mount has:
- `prefix`: VFS path prefix (e.g., `/`, `/workspace`, `/data`)
- `host`: absolute host path (must start with `~` or `/`)
- `mode`: `ro` or `rw` (default: `rw`)

Resolution uses longest-prefix match.

### Path Rules

VFS normalizes all paths before routing:
- Trailing slashes stripped (`/foo/` -> `/foo`)
- Empty components collapsed (`/foo//bar` -> `/foo/bar`)
- Dot components resolved (`/foo/./bar` -> `/foo/bar`)
- Parent traversal resolved (`/foo/../bar` -> `/bar`)

Paths that escape VFS root are rejected:
- `/foo/../../etc` -> error (escapes root)

### Symlink Behavior

Symlinks within a mount are followed. If a symlink points outside the mount boundary (e.g., `<root>/tmp -> /tmp`), the kernel follows it.

This is intentional: the user created the symlink, they accept the consequence. This may change in future versions.

### Default Behavior

If no mounts are configured, VFS is disabled. All `fs:*` syscalls return:
```
E_DISABLED: "filesystem access is disabled (no mounts configured)"
```

### Write Check Order

For write operations, checks occur in this order:
1. VFS resolution (does a mount cover this path?)
2. Mount mode (is the mount writable?)
3. Scope check (is caller a head?)

### Config

Mounts are configured in `config.toml`:

```toml
[[vfs.mounts]]
prefix = "/"
host = "~/github/ianzepp/abbot"
# mode defaults to "rw"

[[vfs.mounts]]
prefix = "/reference"
host = "~/docs"
mode = "ro"
```

Rules:
- `host` must start with `~` (home dir) or `/` (absolute)
- Relative paths are rejected at startup
- Duplicate prefixes are rejected at startup
- Host paths are not validated at startup (fail at runtime if missing)

### SyscallContext Changes

Phase 2 removes from `SyscallContext`:
- `workspace_root`
- `validate_path()`

All path validation moves to VFS.

## Integration Plan

Order of work:
1. Introduce `vfs` module with `MountTable` and `HostMount`.
2. Add VFS initialization to kernel boot, reading from `config.toml`.
3. Update `fs:*` syscall handlers to use VFS resolution instead of `validate_path()`.
4. Add mutation check to mutating syscalls (fs:write, proc:run, etc.):
   - parse scope to determine caller type
   - reject if caller is not `head/*`
5. Remove `workspace_root` and `validate_path()` from `SyscallContext`.

Acceptance criteria:
- All `fs:*` syscalls route through VFS.
- No mounts configured = `E_DISABLED` on all `fs:*` calls.
- A `hand/*` caller cannot execute mutating syscalls.
- A `head/*` caller cannot write to a `ro` mount.
- Paths that escape VFS root are rejected.
- No complex policy knobs are required to use the system.
