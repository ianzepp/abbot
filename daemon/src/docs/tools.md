# Tools

Tools are the LLM-facing interface to syscalls. Each agent type (head, hand, mind) has a curated set of tools available.

## Head Tools

Head agents have access to planning and coordination tools:

- `head__task_create` — Delegate work to a hand agent.
- `head__memory_recall` — Search long-term memory.
- `head__state_query` — Introspect system state (wants, logs, stats).
- `head__fs_read_excerpt` — Read a file excerpt (limited lines).
- `head__fs_list_brief` — List files in workspace.
- `head__config_read` / `head__config_update` — Read/write configuration.
- `head__docs_list` / `head__docs_search` / `head__docs_read` — Access documentation.

## Hand Tools

Hand agents have direct execution tools:

- `hand__fs_list` / `hand__fs_search` / `hand__fs_read` — Filesystem access.
- `hand__fs_diff` — Diff files.
- `hand__text_echo` — Echo text output.
- `hand__http_get` — HTTP requests.

## Tool Effects

Tools are classified by effect:

- **ReadOnly** — No side effects (e.g., reading files, listing docs).
- **Mutating** — Changes state (e.g., creating tasks, writing files).
