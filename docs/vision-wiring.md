## Vision: Wiring Core Pieces (Config, Workspace, VFS, Kernel)

This document specifies how Abbot wires together configuration, workspace layout, VFS, and the kernel syscall layer now that the core pieces exist.

The intent is to discard the legacy "sandbox" concept (which previously tried to constrain filesystem access) and make VFS + mount modes + scope-based mutation the single policy boundary.

### Goals

- Single canonical runtime config file.
- One workspace directory per running instance (explicitly configured).
- VFS is the filesystem boundary (not sandboxes).
- Kernel syscalls are the execution backend for built-in tools (one policy surface).
- Clear separation of agent-visible files (VFS-mounted) vs private state (DBs, memory).

### Non-Goals

- Multi-profile/workspace selection from a single config file.
- Automatic migration of legacy sandbox data.
- Folding `models.toml` into the main config.
- Full cancellation plumbed through head tool execution (hands already support cancellation).

---

## 1) Configuration

### Canonical config path

- Default: `~/.config/abbot/abbot.toml`
- Override: `abbot --config /path/to/abbot.toml`

`models.toml` remains separate at `~/.config/abbot/models.toml`.

### Required config field

Top-level:

```toml
workspace = "/absolute/host/path/to/workspace"
```

If `workspace` is missing/empty, `abbot run` must error out with a clear message.

### VFS mounts in config

Existing `[[vfs.mounts]]` remains supported:

```toml
[vfs]
[[vfs.mounts]]
prefix = "/deps"
host = "/absolute/host/path/to/deps"
mode = "ro"
```

`models.toml` continues to define model IDs, base URLs, and API key resolution.

---

## 2) Workspace Layout

The configured `workspace` directory is the private "app home".

Structure:

```
<workspace>/
  root/           # agent-visible filesystem (mounted at VFS '/'
  store.db        # Abbot store database
  recall.db       # recall/search database
  ems.db          # EMS database
  mind/           # markdown memory (private by default)
  head/<id>/      # markdown memory (private by default)
```

Notes:

- Only `<workspace>/root/` is mounted by default.
- DBs and memory files live outside VFS unless explicitly mounted.

---

## 3) Boot Behavior

On `abbot run`:

1. Resolve config path (CLI `--config` or default).
2. Load config and initialize global config (`AppConfig::init(path)`), then initialize models config.
3. Validate `workspace`:
   - If `<workspace>/` does not exist: error.
   - If `<workspace>/root/` does not exist: create it.
4. Set process current directory (CWD) to `<workspace>/root/`.
5. Open DBs at:
   - `<workspace>/store.db`
   - `<workspace>/recall.db`
   - `<workspace>/ems.db`
6. Initialize kernel + VFS (see next section).
7. Start services (Mind/Head/Hand/Server/etc.) using this workspace + DBs.

`abbot init`:

- Creates config files only (main config + models.toml), does not create workspaces.

---

## 4) VFS Wiring

### Default mount

Abbot must auto-configure a default VFS mount:

- `prefix = "/"`
- `host = "<workspace>/root"`
- `mode = "rw"`

### Interaction with config mounts

- If config includes a mount with `prefix = "/"`, it overrides the auto-mount.
- Otherwise, the auto-mount is included.
- All other configured mounts are appended.

This yields: FS syscalls work out-of-the-box while preserving the ability to lock down mounts.

---

## 5) Kernel as the Execution Backend

### Principle

Built-in tools must not directly read/write host filesystem, spawn processes, run git, or perform network I/O.
They must issue kernel syscall requests so that:

- VFS policy is enforced centrally.
- Mount RO/RW is enforced centrally.
- Head-only mutation is enforced centrally.
- Deadlines and cancellation behavior are consistent.

### Tool to syscall mapping

Built-in tool implementations in `src/agent_tools.rs` should dispatch to syscalls:

- `read_file` -> `fs:read`
- `write_file` -> `fs:write`
- `git` -> `git:run`
- `curl` -> `net:fetch`
- (any process tool) -> `proc:run`

### Scope + mutation

- Syscalls enforce mutation via scope:
  - `head/<head_id>` can mutate.
  - `hand/<hand_id>` is read-only.

To support auditability and future policy, scopes must include real IDs.

### Path semantics

- Syscalls require absolute VFS paths (starting with `/`).
- `agent_tools` is responsible for converting user-relative paths into absolute VFS paths.
  - Conversion is based on the current host CWD (which is set to `<workspace>/root/` at boot).

---

## 6) Removing Sandboxes

The legacy sandbox concept is removed entirely:

- Remove `--sandbox` flag.
- Remove sandbox-related CLI subcommands (`sandbox`, `mount`, `export`, etc.).
- Delete sandbox path helpers and config/env creation functions.
- Drop `root.env` support.

Abbot now runs against a single configured workspace.

---

## 7) Compatibility / Migration

No automatic migration is performed.

If a user wants to reuse an old sandbox directory, they can set:

```toml
workspace = "/path/to/old/sandbox-dir"
```

but Abbot will not attempt to rename or import old DB filenames.
This change assumes there is nothing of value to preserve.

---

## 8) Implementation Checklist

### Config and boot

- Ensure daemon boot calls `AppConfig::init(...)` before starting services.
- Add `workspace` to config parsing.
- Update boot logic to:
  - validate `<workspace>` exists
  - create `<workspace>/root` if missing
  - set CWD to `<workspace>/root`
  - open DBs using `.db` filenames
- Ensure kernel init uses the final mount set (auto + config).

### Kernel routing

- Modify `agent_tools` built-ins to call kernel syscalls.
- Plumb real agent IDs into syscall scopes:
  - Update `exec_hand_tool` signature to accept `hand_id`.
  - Head tools use `head/<head_id>`.
- Require absolute VFS paths for syscalls; implement path conversion at tool layer.

### Deletions

- Remove sandbox CLI and helpers.
- Remove `root.env` and sandbox metadata file creation.

---

## 9) Testing

- Unit tests for:
  - mount assembly (auto-mount + override behavior)
  - absolute VFS path requirement and conversion
  - mutation enforcement via scope with real IDs
- Integration tests for:
  - boot with config-defined workspace
  - fs read/write through tool surface -> kernel -> VFS
  - git/curl/proc through tool surface -> kernel

---

## 10) Open Items

- Whether server `--addr` should be config-first or CLI-first (deferred).
- Whether to provide an explicit migration command (deferred; default is no migration).
