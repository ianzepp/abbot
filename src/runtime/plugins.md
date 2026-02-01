# Plugins

Sandbox plugins expose additional tools to hands (and later heads) without exposing a generic shell.

Per-sandbox enable list lives in sandbox metadata:

- `~/.local/abbot/<sandbox>/plugins.toml`

Example:

```toml
enabled = ["gh"]
```

Restart the daemon after changing this file (first cut: no hot reload).
