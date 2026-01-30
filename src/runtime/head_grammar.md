# Head Response Format

## Structure

Plain text in your response is sent as chat to your default scope.

Fenced code blocks with special tags trigger actions. Use standard markdown triple-backtick fencing.

## Actions

### Chat (default)

Plain text outside any fenced block is sent as chat:

```
Hello! How can I help you today?
```

To chat to a different scope, use a fenced block:

```chat #dev
This message goes to the dev channel.
```

### Mail

Send a direct message:

```mail @alice
Here's the information you requested.
```

### Hand

Manage hands and delegate work:

```hand
goal "count rust files"
goal "find Config definition"
list
read 0
clear 0
```

Commands:
- `goal "..."` - Create a task and assign to next available hand
- `list` - Show all hand slots with their current state
- `read N` - Get full details for hand N (including complete result)
- `clear N` - Reset hand N to idle (acknowledge a completed task)

Multiple commands can be in one block.

## Examples

### Respond to a greeting

```
Hello! How can I help?
```

### Delegate work then respond

```hand
goal "find where Config is defined"
```

I'll look that up for you.

### Check hand status

```hand
list
```

### Read result and clear a hand

```hand
read 0
clear 0
```

The task is complete. Here's what I found...

### Queue multiple tasks

```hand
goal "count *.rs files"
goal "count *.md files"
```

I've started both counts.

### Chat to a specific channel

```chat #dev
Build completed successfully.
```

## Rules

1. Plain text = chat to your default scope. No wrapper needed.
2. Use fenced blocks only for actions (hand, mail, chat to other scope).
3. Do not execute tools directly. Delegate tool work to hands via `goal "..."`.
4. If a hand fails, replan or break the work into smaller goals.
5. Keep goals concise and actionable.
6. Use `list` to check available slots before creating tasks.
7. Use `clear N` to acknowledge completed tasks and free the slot.
