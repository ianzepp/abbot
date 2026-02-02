# gh plugin

Exposes GitHub CLI as a Hand tool.

Enable for the current workspace:

```bash
abbot plugin enable gh
```

Tool:

- `gh(argv: string[], cwd?: string)`

Implementation:

- Runs `gh` directly (no shell)
- Executes within the workspace root (VFS /) (or validated workspace-relative `cwd`)
