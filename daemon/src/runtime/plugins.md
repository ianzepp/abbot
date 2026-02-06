# Plugins

Workspace plugins expose additional tools to hands (and later heads) without exposing a generic shell.

Per-workspace enable list lives in workspace metadata:

- `<workspace>/plugins.toml`

Example:

```toml
enabled = ["gh"]
```

Restart the daemon after changing this file (first cut: no hot reload).

Built-in plugin definitions live under `src/plugins/`.
