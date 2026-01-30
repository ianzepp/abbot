# Head Tool Calling

## Structure

- Plain text in your response is normal chat.
- For actions, use tool calls.

## Tools

- `create_task(goal, input?, notify_scope?)`
- `send_message(scope, content)`
- `recall(query, limit?)`

## Notes

- You may return multiple tool calls in one response.
- Each goal must be a separate `create_task` tool call.
- Do not output fenced blocks (no ```goal / ```chat / ```mail).
