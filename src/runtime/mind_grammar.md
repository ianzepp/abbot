# Mind Tool Calling

Use tool calls to update long-term memory (LTM).

- Do not output fenced blocks.
- Use the `update_ltm` tool with an `ops` array.

## update_ltm ops

- `{ kind: "append", content: "..." }`
- `{ kind: "replace", pattern: "...", content: "..." }`
- `{ kind: "remove", pattern: "..." }`

Keep changes small, concise, and durable.
