# CLI

The `abbot` CLI manages configuration, providers, and daemon interaction. Commands are either **offline** (no daemon required) or **RPC** (connect to the daemon via Unix socket).

## Global Flags

| Flag | Default | Purpose |
|------|---------|---------|
| `--config <PATH>` | `~/.config/abbot/abbot.toml` | Config file path |
| `--sock <PATH>` | `<workspace>/rpc.sock` | RPC unix socket path |
| `--addr <HOST:PORT>` | From config | API server address (monitor/tui) |
| `--format <auto\|json\|pretty>` | `auto` | Output format (auto = pretty for TTY, JSON for pipes) |
| `--timeout <SECONDS>` | `30` | RPC request timeout |

## Offline Commands

### `abbot config`

Read and write `~/.config/abbot/abbot.toml`.

| Subcommand | Arguments | Purpose |
|------------|-----------|---------|
| `get` | `[SECTION]` | Show all config or a specific section |
| `set` | `SECTION KEY=VALUE...` | Set values within a section |

Sections: `head`, `hand`, `mind`, `server`, `pool`, `harness`, `prompt_cache`, `vfs`, `providers`.

Empty value (`key=`) deletes the key. Values are type-inferred (bool, int, float, string).

### `abbot info`

Show system configuration, workspace state, database sizes, agent config, memory file previews, and provider connectivity tests.

### `abbot service`

| Subcommand | Purpose |
|------------|---------|
| `install` | Install as system service (launchd/systemd) |
| `uninstall` | Remove the service |
| `start` | Start the service |
| `stop` | Stop the service |
| `status` | Show service status and PID |
| `preflight` | Show last preflight check results |

### `abbot reset`

Delete workspace state (databases, memory, head state). Prompts for confirmation unless `--force` is passed. `--config` also deletes and regenerates the config file.

### `abbot providers`

| Subcommand | Arguments | Purpose |
|------------|-----------|---------|
| `refresh` | | Fetch and cache model lists from all providers |
| `list` | | Show cached providers and model counts |
| `models` | `PROVIDER [--limit N]` | Show models for a provider (default limit: 20) |
| `login` | `PROVIDER` | Interactive: open browser, paste API key |
| `add` | `PROVIDER` | Interactive: configure API key without browser |
| `remove` | `PROVIDER` | Remove a provider's API key |
| `test` | | Test connectivity for all providers |
| `use` | `MODEL` | Set model for head/hand/mind (format: `provider/model`) |

Providers: `anthropic`, `openai`, `openrouter`, `ollama`.

### `abbot frames`

| Subcommand | Arguments | Purpose |
|------------|-----------|---------|
| `get` | `ID` | Retrieve a single frame by UUID |
| `replay` | `[KIND] [--limit N]` | Replay recent frames (default limit: 20, excludes ticks) |

### `abbot monitor`

Stream frames from the daemon in real-time via WebSocket. `--filter PATTERN` filters by kind/name (e.g., `chat:*`, `need:*`).

### `abbot tui`

Launch the terminal UI. Passes additional arguments through to `abbot-tui`.

## RPC Commands

All RPC commands require a running daemon and connect via Unix socket.

### `abbot status`

| Subcommand | Purpose |
|------------|---------|
| `info` | Runtime status: uptime, queue depths, active agents, pool sizes |

### `abbot chat`

| Subcommand | Arguments | Purpose |
|------------|-----------|---------|
| `send` | `MESSAGE [--scope SCOPE]` | Send a chat message (default scope: `main`) |

### `abbot need`

| Subcommand | Purpose |
|------------|---------|
| `list` | List queued and active needs |

### `abbot task`

| Subcommand | Purpose |
|------------|---------|
| `list` | List queued and active tasks |

### `abbot room`

| Subcommand | Purpose |
|------------|---------|
| `list` | List active rooms |

### `abbot audit`

| Subcommand | Arguments | Purpose |
|------------|-----------|---------|
| `replay` | `[--limit N] [--name FILTER] [--op FILTER]` | Replay recent audit frames (default limit: 50) |
| `get` | `ID` | Retrieve a single frame by UUID |

## Config Files

| File | Purpose |
|------|---------|
| `~/.config/abbot/abbot.toml` | User/global configuration |
| `~/.config/abbot/keys.env` | API keys (loaded as env vars) |
| `~/.config/abbot/providers/*.json` | Cached provider model lists |
| `<workspace>/config.toml` | Workspace-scoped overrides |

## Output Formats

- **auto** (default) — pretty for TTY, JSON for pipes
- **json** — one compact JSON object per line (NDJSON), suitable for `jq`
- **pretty** — human-readable key: value layout
