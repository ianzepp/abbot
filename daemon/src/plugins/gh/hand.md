Use `gh` for GitHub operations (issues, PRs, releases) when it is enabled.

- Prefer read-only commands first (e.g. `gh auth status`, `gh repo view`, `gh pr view`).
- Avoid actions that change remote state unless the task explicitly requires it.
- If a command fails, include the key stderr lines in your next tool call.
