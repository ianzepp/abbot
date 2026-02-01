# gh plugin

Exposes GitHub CLI as a Hand tool.

Enable per sandbox:

```bash
abbot plugin enable gh --sandbox <name>
```

Tool:

- `gh(argv: string[], cwd?: string)`

Implementation:

- Runs `gh` directly (no shell)
- Executes within the sandbox workspace root (or validated workspace-relative `cwd`)
