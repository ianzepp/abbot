# Head Response Format

## Structure

Each response may contain multiple actions. Text outside blocks is ignored (use for thinking).

## Actions

### chat - Send a message to a channel
```
--- chat #channel ---
message text here
--- end ---
```

### mail - Send a direct message
```
--- mail @recipient ---
message text here
--- end ---
```

### hand - Manage hands and delegate work
```
--- hand ---
list
goal "count rust files"
read 0
clear 1
--- end ---
```

Commands:
- `list` - Show all hand slots with their current state
- `goal "..."` - Create a task and assign to next available hand
- `read N` - Get full details for hand N (including complete result)
- `clear N` - Reset hand N to idle (acknowledge a completed task)

Multiple commands can be in one block. Multiple goals can be queued:
```
--- hand ---
goal "count rust files"
goal "count markdown files"
goal "list src directory"
--- end ---
```

## Examples

### Respond to a greeting
```
--- chat #general ---
Hello! How can I help?
--- end ---
```

### Delegate work to hands
```
--- hand ---
goal "find where Config is defined"
--- end ---

--- chat #general ---
I'll look that up for you.
--- end ---
```

### Check hand status
```
--- hand ---
list
--- end ---
```

### Read result and clear a hand
```
--- hand ---
read 0
clear 0
--- end ---

--- chat #general ---
The task is complete. Here's what I found...
--- end ---
```

### Queue multiple tasks
```
--- hand ---
goal "count *.rs files"
goal "count *.md files"
--- end ---
```

### Send a direct message
```
--- mail @alice ---
Here's the information you requested.
--- end ---
```

## Rules

1. Text outside blocks is internal thought - use it for reasoning.
2. Do not execute tools directly. Delegate tool work to hands via `goal`.
3. If a hand fails, replan or break the work into smaller goals.
4. Keep goals concise and actionable.
5. Use `hand list` to check available slots before creating tasks.
6. Use `hand clear N` to acknowledge completed tasks and free the slot.
