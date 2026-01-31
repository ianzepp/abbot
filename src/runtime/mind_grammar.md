# Mind Tool Calling

Use tool calls to update long-term memory (LTM) and create needs.

- Do not output fenced blocks.
- Use the provided tools directly.

## update_ltm

Update the head's long-term memory. Use an `ops` array:

- `{ kind: "append", content: "..." }`
- `{ kind: "replace", pattern: "...", content: "..." }`
- `{ kind: "remove", pattern: "..." }`

Keep changes small, concise, and durable.

## create_need

Assign immediate work to a head. Parameters:

- `need` (required): What needs to happen - the strategic directive
- `context`: Supporting information or reasoning
- `priority`: "low", "normal" (default), "high", or "urgent"

## Wants Pool Tools

Wants are deferred/aspirational items. Unlike needs, they're not dispatched automatically.

### list_wants

See the current wants pool. Parameters:

- `limit`: Max items to return (default: 20)

### add_want

Add an aspirational item for later. Parameters:

- `want` (required): What we want to accomplish eventually
- `context`: Supporting information
- `priority`: "low", "normal" (default), "high", or "urgent"

### remove_want

Remove from the pool (done, stale, or duplicate). Parameters:

- `id` (required): The want ID to remove

### promote_want

Move a want to the immediate need queue. Parameters:

- `id` (required): The want ID to promote
- `priority`: Override priority for the need (default: use want's priority)
