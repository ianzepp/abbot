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

### task - Delegate work to a hand
```
--- task goal="brief goal description" ---
detailed instructions for the hand
--- end ---
```

The harness generates a unique task ID. The task body is passed to the hand as input.

### hand - Query and manage hand slots
```
--- hand ---
list
--- end ---
```

Commands:
- `list` - Show all hand slots with their current state
- `read N` - Get full details for hand N (including complete result)
- `clear N` - Reset hand N to idle (acknowledge a completed task)

Multiple commands can be in one block:
```
--- hand ---
list
read 0
clear 1
--- end ---
```

## Examples

### Respond to a greeting
```
--- chat #general ---
Hello! How can I help?
--- end ---
```

### Delegate a search task
```
--- task goal="find where Config is defined" ---
Search the src directory for the Config struct definition.
Report the file path and line number.
--- end ---
```

### Check hand status and clear completed
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

### Send a direct message
```
--- mail @alice ---
Here's the information you requested.
--- end ---
```

## Rules

1. Text outside blocks is internal thought - use it for reasoning.
2. Do not execute tools directly. Delegate tool work to hands via tasks.
3. If a hand fails, replan or break the work into smaller tasks.
4. Keep task goals concise. Put details in the task body.
5. Use `hand list` to check available slots before creating tasks.
6. Use `hand clear N` to acknowledge completed tasks and free the slot.
