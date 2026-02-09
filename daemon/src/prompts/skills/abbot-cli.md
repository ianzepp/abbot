---
name: abbot-cli
description: Abbot daemon CLI for setup, lifecycle, monitoring, and data management via exec:run
category: system
requires:
  - exec
---

# Abbot CLI

Use `exec:run` with `program: "abbot"` to manage the Abbot daemon, configuration, providers, entities, and diagnostics.

## Calling Convention

```json
{ "program": "abbot", "args": ["status"] }
{ "program": "abbot", "args": ["config", "get", "head"] }
{ "program": "abbot", "args": ["chat", "send", "Hello, how are you?"] }
```

## Global Options

All commands support these flags:

| Flag | Description | Default |
|------|-------------|---------|
| `--config <PATH>` | Path to config file | `~/.abbot/abbot.toml` |
| `--sock <PATH>` | Path to RPC unix socket | `~/.abbot/rpc.sock` |
| `--addr <HOST:PORT>` | API server address | — |
| `--format <FORMAT>` | Output: `auto`, `pretty`, `json` | `auto` |
| `--timeout <SECONDS>` | RPC request timeout | `30` |

## Offline vs Online

Some commands work without a running daemon (offline), others require the daemon to be running (RPC).

- **Offline**: `init`, `config`, `providers`, `use`, `mounts`, `reset`, `doctor`, `info`, `frames`, `service`, `scripts`
- **RPC** (daemon required): `chat`, `ems`
- **Hybrid**: `status`, `tail`, `start`, `stop`, `restart`, `run`

---

## Setup & Initialization

### First-time setup

```json
{ "program": "abbot", "args": ["init"] }
```

### Re-initialize (clean slate)

```json
{ "program": "abbot", "args": ["init", "--clean"] }
```

### Non-interactive setup with defaults

```json
{ "program": "abbot", "args": ["init", "--accept-defaults", "-p", "anthropic"] }
```

### Specify provider and model

```json
{ "program": "abbot", "args": ["init", "-p", "openai", "-m", "gpt-4.1"] }
```

### Developer/dogfood mode

```json
{ "program": "abbot", "args": ["init", "--developer"] }
```

---

## Configuration

### Show all configuration

```json
{ "program": "abbot", "args": ["config", "get"] }
```

### Show a specific section

```json
{ "program": "abbot", "args": ["config", "get", "head"] }
```

Sections: `head`, `hand`, `mind`, `server`, `pool`, `harness`, and others defined in `abbot.toml`.

### Set values in a section

```json
{ "program": "abbot", "args": ["config", "set", "head", "model=anthropic/claude-sonnet-4-20250514", "temperature=0.7"] }
```

Multiple key=value pairs can be set in one call.

---

## Provider Management

### List cached providers

```json
{ "program": "abbot", "args": ["providers", "list"] }
```

### Refresh model lists from all providers

```json
{ "program": "abbot", "args": ["providers", "refresh"] }
```

### Show models for a provider

```json
{ "program": "abbot", "args": ["providers", "models", "anthropic"] }
```

With a limit:

```json
{ "program": "abbot", "args": ["providers", "models", "openrouter", "-l", "50"] }
```

### Add a provider (API key)

```json
{ "program": "abbot", "args": ["providers", "add", "anthropic"] }
```

### Open browser to get API key

```json
{ "program": "abbot", "args": ["providers", "login", "openai"] }
```

### Remove a provider

```json
{ "program": "abbot", "args": ["providers", "remove", "ollama"] }
```

### Test all provider connections

```json
{ "program": "abbot", "args": ["providers", "test"] }
```

### Switch all configs to a specific model

```json
{ "program": "abbot", "args": ["providers", "use", "anthropic/claude-sonnet-4-20250514"] }
```

### Quick model switch

```json
{ "program": "abbot", "args": ["use", "anthropic", "claude-sonnet-4-20250514"] }
```

---

## VFS Mounts

### List mounts

```json
{ "program": "abbot", "args": ["mounts", "list"] }
```

### Add a mount

```json
{ "program": "abbot", "args": ["mounts", "add", "/projects", "~/github/myorg"] }
```

### Add a read-only mount

```json
{ "program": "abbot", "args": ["mounts", "add", "/data", "/shared/datasets", "--ro"] }
```

### Remove a mount

```json
{ "program": "abbot", "args": ["mounts", "remove", "/projects"] }
```

---

## Daemon Lifecycle

### Start the daemon

```json
{ "program": "abbot", "args": ["start"] }
```

### Stop the daemon

```json
{ "program": "abbot", "args": ["stop"] }
```

### Restart the daemon

```json
{ "program": "abbot", "args": ["restart"] }
```

### Check daemon status

```json
{ "program": "abbot", "args": ["status"] }
```

### Reset workspace state

```json
{ "program": "abbot", "args": ["reset", "--force"] }
```

---

## System Service

### Install as system service

```json
{ "program": "abbot", "args": ["service", "install"] }
```

macOS: launchd plist. Linux: systemd unit.

### Uninstall system service

```json
{ "program": "abbot", "args": ["service", "uninstall"] }
```

---

## Diagnostics

### Health check (offline)

```json
{ "program": "abbot", "args": ["doctor"] }
```

Runs preflight validation: checks config, providers, API keys, databases.

### System info

```json
{ "program": "abbot", "args": ["info"] }
```

Shows paths, databases, configuration, providers, traits, and runtime status.

---

## Chat

### Send a message

```json
{ "program": "abbot", "args": ["chat", "send", "What tasks are pending?"] }
```

### Send to a specific scope

```json
{ "program": "abbot", "args": ["chat", "send", "Review the auth module", "--scope", "code-review"] }
```

---

## Entity Management (EMS)

### List queued and active needs

```json
{ "program": "abbot", "args": ["ems", "needs"] }
```

### List queued and active tasks

```json
{ "program": "abbot", "args": ["ems", "tasks"] }
```

---

## Frame History

### Replay recent frames

```json
{ "program": "abbot", "args": ["frames", "replay"] }
```

### Replay with a limit

```json
{ "program": "abbot", "args": ["frames", "replay", "--limit", "50"] }
```

### Replay by event kind

```json
{ "program": "abbot", "args": ["frames", "replay", "chat:user"] }
```

### Filter by syscall name

```json
{ "program": "abbot", "args": ["frames", "replay", "--name", "ems"] }
```

### Filter by frame op

```json
{ "program": "abbot", "args": ["frames", "replay", "--op", "error"] }
```

Ops: `req`, `ok`, `error`, `event`, `item`.

### Replay since a specific frame sequence

```json
{ "program": "abbot", "args": ["frames", "replay", "--since-frame", "1500"] }
```

### Get a specific frame by UUID

```json
{ "program": "abbot", "args": ["frames", "get", "a1b2c3d4-e5f6-7890-abcd-ef1234567890"] }
```

### Stream live frames

```json
{ "program": "abbot", "args": ["tail"] }
```

### Stream with a filter

```json
{ "program": "abbot", "args": ["tail", "--filter", "chat:*"] }
```

---

## Debug Bundles (Scripts)

Render the prompt bundles that agents would see, without running the daemon.

### Show mind loop bundle

```json
{ "program": "abbot", "args": ["scripts", "mind"] }
```

With a specific channel:

```json
{ "program": "abbot", "args": ["scripts", "mind", "--channel", "#dev"] }
```

### Show head agent bundle

```json
{ "program": "abbot", "args": ["scripts", "head"] }
```

With a specific room and head identity:

```json
{ "program": "abbot", "args": ["scripts", "head", "--room", "#main", "--head-id", "Abbot"] }
```

### Show hand agent bundle

```json
{ "program": "abbot", "args": ["scripts", "hand", "--task-id", "42", "--prompt", "Fix the login bug"] }
```

---

## External Tools (Proxy Mode)

Launch external tools with Abbot as their API provider.

### Launch Claude Code through Abbot

```json
{ "program": "abbot", "args": ["run", "claude"] }
```

With additional arguments:

```json
{ "program": "abbot", "args": ["run", "claude", "--", "--model", "claude-sonnet-4-20250514"] }
```

### Launch OpenCode through Abbot

```json
{ "program": "abbot", "args": ["run", "opencode"] }
```

### Launch the monitoring dashboard

```json
{ "program": "abbot", "args": ["run", "monitor"] }
```

### Launch the TUI

```json
{ "program": "abbot", "args": ["tui"] }
```

Or equivalently:

```json
{ "program": "abbot", "args": ["run", "tui"] }
```

---

## Output Format

Use `--format json` for machine-readable output that can be parsed programmatically:

```json
{ "program": "abbot", "args": ["--format", "json", "status"] }
{ "program": "abbot", "args": ["--format", "json", "ems", "tasks"] }
{ "program": "abbot", "args": ["--format", "json", "frames", "replay", "--limit", "10"] }
```

`--format auto` (default) uses pretty output for terminals, JSON for pipes.

---

## File Paths

| File | Path | Purpose |
|------|------|---------|
| Config | `~/.abbot/abbot.toml` | Main configuration |
| API keys | `~/.abbot/keys.env` | Provider API keys |
| RPC socket | `~/.abbot/rpc.sock` | Daemon communication |
| Store | `~/.abbot/store.db` | Conversation history |
| Entities | `~/.abbot/ems.db` | Tasks, needs, wants, memories |
| Frames | `~/.abbot/frames.db` | Frame audit log |
| Preflight | `~/.abbot/preflight.log` | Health check results |

---

## Common Workflows

### Initial setup from scratch

1. Initialize:
   `abbot init -p anthropic`
2. Add a project mount:
   `abbot mounts add /projects ~/github/myorg`
3. Install as service:
   `abbot service install`
4. Start:
   `abbot start`
5. Verify:
   `abbot status`

### Switch models

1. Check available models:
   `abbot providers models anthropic`
2. Switch:
   `abbot use anthropic claude-sonnet-4-20250514`
3. Verify:
   `abbot config get head`

### Diagnose issues

1. Run health check:
   `abbot doctor`
2. Check system info:
   `abbot info`
3. Check provider connectivity:
   `abbot providers test`
4. Review recent errors:
   `abbot frames replay --op error --limit 20`

### Monitor daemon activity

1. Check status:
   `abbot status`
2. Stream live activity:
   `abbot tail`
3. Review recent frames:
   `abbot frames replay --limit 50`
4. Check entity queues:
   `abbot ems needs` / `abbot ems tasks`

---

## Safety Notes

- **Read-only** (safe for hand agents): `status`, `info`, `doctor`, `config get`, `providers list/models/test`, `mounts list`, `frames get/replay`, `ems needs/tasks`, `scripts mind/head/hand`
- **Mutating** (requires head/mind): `init`, `config set`, `providers add/remove/use/login/refresh`, `mounts add/remove`, `use`, `start`, `stop`, `restart`, `reset`, `service install/uninstall`, `chat send`
- `init --clean` and `reset --force` are destructive — they delete databases and state.
- `service uninstall` removes the system service definition.
- `abbot` is in the default exec allowlist.
