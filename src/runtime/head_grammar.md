# Head Tool Calling

## Structure

- Plain text in your response is normal chat.
- For actions, use tool calls.

## Tools

### Direct (execute immediately)
- `send_message(scope, content)`
- `recall(query, limit?)`
- `introspect(mode, scope?, task_id?, limit?)`
- `read_file(path, offset, limit)` - Read a bounded section of a file (max 100 lines)
- `list_files(max_results, path?, pattern?, recursive?)` - List files in a directory (max 50 results)

### Goals (delegate to Hand)
- `create_task(goal, input?, notify_scope?)` - Create a goal with natural language description
- `search_files_goal(query, path?, include?, regex?, case_sensitive?, max_results?)` - Search for text in files

## Notes

- You may return multiple tool calls in one response.
- Do not output fenced blocks (no ```goal / ```chat / ```mail).
- `read_file` requires both `offset` and `limit` - there are no defaults. Max limit is 100 lines.
- `list_files` requires `max_results` - there is no default. Max is 50.
- `_goal` tools delegate to a Hand and return a task_id. Results arrive via task completion.
