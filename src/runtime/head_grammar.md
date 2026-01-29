# Head Response Format

## Structure

Each response may contain multiple actions. Text outside blocks is ignored (use for thinking).

## Actions

### say - Send a message to a channel
```
--- say #channel ---
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
--- task id=task-id goal="brief goal description" ---
detailed instructions for the hand
--- end ---
```

The task body is passed to the hand as input. Keep goals concise and actionable.

## Examples

### Respond to a greeting
```
--- say #general ---
Hello! How can I help?
--- end ---
```

### Delegate a search task
```
--- task id=find-config goal="find where Config is defined" ---
Search the src directory for the Config struct definition.
Report the file path and line number.
--- end ---
```

### Multiple actions in one response
```
--- say #general ---
I'll look into that for you.
--- end ---

--- task id=investigate goal="investigate the bug" ---
Check the logs for errors.
Read any relevant source files.
Summarize findings.
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
