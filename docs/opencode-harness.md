# OpenCode as Abbot Harness

OpenCode can serve as a TUI and tool executor for abbot, with abbot acting as the backend "brain" via an OpenAI-compatible API.

## Architecture

```
User <-> OpenCode (TUI + tools) <-> Abbot API (/v1/chat/completions) <-> Abbot Head
```

- **OpenCode**: Provides the UI and executes tools in the user's environment
- **Abbot**: Provides reasoning, memory, goals, and internal tool execution

## OpenAI API Protocol

OpenCode sends requests to `/v1/chat/completions` with:

1. **messages** - Array containing:
   - System message (OpenCode's own instructions)
   - Conversation history (all previous user/assistant turns)
   - Current user message

2. **tools** - Array of tool definitions (name, description, parameters)

3. **stream** - Boolean for SSE streaming

This entire payload is sent on **every request** (stateless protocol).

## Tools Provided by OpenCode

| Tool | Purpose |
|------|---------|
| `bash` | Execute shell commands in user's working directory |
| `read` | Read file contents |
| `write` | Write/overwrite files |
| `edit` | Perform string replacements in files |
| `glob` | Find files by pattern |
| `grep` | Search file contents |
| `question` | Ask user for input/clarification |
| `task` | Launch subagents (general, explore) |
| `webfetch` | Fetch URL contents |
| `todowrite` | Create/update task list |
| `todoread` | Read current task list |
| `skill` | Load specialized skill instructions |

## Two Execution Contexts

1. **Abbot's internal context**: Memory, goals, reflection, hand tasks - runs inside abbot process
2. **User's context**: File operations, shell commands - runs in OpenCode's working directory

The challenge: abbot needs to distinguish which context a tool should run in.

## Current Limitations

- Abbot ignores OpenCode's system message (uses its own head instructions)
- Abbot ignores the tools array (uses internal grammar for actions)
- Abbot's output grammar (`<think>`, `<chat>`, `<goal>`) is not understood by OpenCode
- No support for OpenAI `tool_calls` response format

## Integration Path

For full integration, abbot would need to:

1. Parse the incoming `tools` array to understand available user-context tools
2. Distinguish internal operations (memory, goals) from user-facing operations (file edits, bash)
3. Emit `tool_calls` in the OpenAI response format for user-context operations
4. Handle tool results coming back as tool-role messages
5. Continue reasoning with tool results incorporated

This creates a translation layer between abbot's internal grammar and the OpenAI tool calling protocol.

## Configuration

Register abbot as an OpenCode provider:

```bash
abbot opencode register
```

Run OpenCode with abbot backend:

```bash
abbot opencode run
```

Requires abbot server running on `http://localhost:8080`.
