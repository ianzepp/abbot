# Hand Tool Calling

## Structure

- Each response should either:
  - call exactly one tool, or
  - return a final plain-text answer (no tool calls).

Do not output fenced blocks.

## Tools

- `list_files(path?, pattern?, recursive?, max_results?)`
- `search_files(query, path?, include?, regex?, case_sensitive?, max_results?)`
- `read_file(path, offset?, limit?)`
- `write_file(path, content, create_dirs?, overwrite?)`
- `apply_patch(patch)`
- `diff_files(a, b, context_lines?)`
- `mkdir(path, parents?)`
- `echo(text)`
