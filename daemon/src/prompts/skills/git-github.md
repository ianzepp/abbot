---
name: git-github
description: Git version control and GitHub CLI workflows via exec:run
category: devops
requires:
  - exec
---

# Git & GitHub CLI

Use `exec:run` with `program: "git"` or `program: "gh"` to perform version control and GitHub operations. All commands execute in the project working directory (set via `cwd` if needed).

## Calling Convention

Every command is an `exec:run` tool call. Arguments are passed as an array of strings — never as a single shell string.

```json
{ "program": "git", "args": ["status", "--short"] }
{ "program": "gh", "args": ["pr", "list", "--state", "open"] }
```

Use `cwd` to target a specific VFS-mounted project path when the workspace has multiple mounts:

```json
{ "program": "git", "args": ["status", "--short"], "cwd": "/projects/backend" }
```

## Read-Only vs Mutating

Hand agents may only run **read-only** commands. Mutating commands (commit, push, merge, PR create, issue close, etc.) require **head or mind** role.

---

## Git: Repository Inspection

### Status

```json
{ "program": "git", "args": ["status", "--short"] }
```

Short flags: `M` = modified, `A` = added, `??` = untracked, `D` = deleted.

### Log

Recent commits (oneline):

```json
{ "program": "git", "args": ["log", "--oneline", "-20"] }
```

With dates and authors:

```json
{ "program": "git", "args": ["log", "--format=%h %ad %an: %s", "--date=short", "-20"] }
```

Commits on a branch since it diverged from main:

```json
{ "program": "git", "args": ["log", "--oneline", "main..HEAD"] }
```

### Diff

Unstaged changes:

```json
{ "program": "git", "args": ["diff"] }
```

Staged changes:

```json
{ "program": "git", "args": ["diff", "--cached"] }
```

Diff between branches:

```json
{ "program": "git", "args": ["diff", "main..feature-branch"] }
```

Summary only (file list with stats):

```json
{ "program": "git", "args": ["diff", "--stat"] }
```

### Show

View a specific commit:

```json
{ "program": "git", "args": ["show", "abc1234"] }
```

Show a file at a specific revision:

```json
{ "program": "git", "args": ["show", "HEAD~3:src/main.rs"] }
```

### Blame

```json
{ "program": "git", "args": ["blame", "-L", "10,30", "src/main.rs"] }
```

---

## Git: Branching

### List branches

```json
{ "program": "git", "args": ["branch", "-a"] }
```

### Current branch

```json
{ "program": "git", "args": ["branch", "--show-current"] }
```

### Create and switch to a new branch

```json
{ "program": "git", "args": ["checkout", "-b", "feature/my-feature"] }
```

### Switch to an existing branch

```json
{ "program": "git", "args": ["switch", "main"] }
```

### Delete a local branch

```json
{ "program": "git", "args": ["branch", "-d", "feature/old-branch"] }
```

### Merge a branch into current

```json
{ "program": "git", "args": ["merge", "feature/my-feature"] }
```

Merge with no fast-forward (preserves branch history):

```json
{ "program": "git", "args": ["merge", "--no-ff", "feature/my-feature"] }
```

---

## Git: Staging & Committing

### Stage specific files

```json
{ "program": "git", "args": ["add", "src/main.rs", "src/lib.rs"] }
```

### Stage all changes

```json
{ "program": "git", "args": ["add", "-A"] }
```

### Unstage a file

```json
{ "program": "git", "args": ["reset", "HEAD", "src/main.rs"] }
```

### Commit

```json
{ "program": "git", "args": ["commit", "-m", "feat: add user authentication"] }
```

Commit with multi-line message (use newlines in the message string):

```json
{ "program": "git", "args": ["commit", "-m", "feat: add user authentication\n\nImplements JWT-based auth with refresh tokens.\nCloses #42."] }
```

### Amend the last commit message

```json
{ "program": "git", "args": ["commit", "--amend", "-m", "fix: correct typo in auth handler"] }
```

---

## Git: Remote Operations

### Fetch

```json
{ "program": "git", "args": ["fetch", "origin"] }
```

### Pull

```json
{ "program": "git", "args": ["pull", "origin", "main"] }
```

Pull with rebase (avoids merge commits):

```json
{ "program": "git", "args": ["pull", "--rebase", "origin", "main"] }
```

### Push

```json
{ "program": "git", "args": ["push", "origin", "feature/my-feature"] }
```

Push and set upstream:

```json
{ "program": "git", "args": ["push", "-u", "origin", "feature/my-feature"] }
```

### Remotes

```json
{ "program": "git", "args": ["remote", "-v"] }
```

---

## Git: Stashing

### Save current changes

```json
{ "program": "git", "args": ["stash", "push", "-m", "wip: auth refactor"] }
```

### List stashes

```json
{ "program": "git", "args": ["stash", "list"] }
```

### Apply most recent stash

```json
{ "program": "git", "args": ["stash", "pop"] }
```

### Apply a specific stash

```json
{ "program": "git", "args": ["stash", "apply", "stash@{2}"] }
```

### Drop a stash

```json
{ "program": "git", "args": ["stash", "drop", "stash@{0}"] }
```

---

## Git: Tags

### List tags

```json
{ "program": "git", "args": ["tag", "--list"] }
```

### Create an annotated tag

```json
{ "program": "git", "args": ["tag", "-a", "v1.0.0", "-m", "Release 1.0.0"] }
```

### Push tags

```json
{ "program": "git", "args": ["push", "origin", "--tags"] }
```

---

## GitHub CLI: Pull Requests

### List open PRs

```json
{ "program": "gh", "args": ["pr", "list", "--state", "open"] }
```

### View PR details

```json
{ "program": "gh", "args": ["pr", "view", "123"] }
```

View PR diff:

```json
{ "program": "gh", "args": ["pr", "diff", "123"] }
```

### Create a PR

```json
{ "program": "gh", "args": ["pr", "create", "--title", "feat: add auth", "--body", "Implements JWT authentication.\n\n## Changes\n- Added login endpoint\n- Added token refresh"] }
```

Create a draft PR:

```json
{ "program": "gh", "args": ["pr", "create", "--title", "wip: auth refactor", "--body", "Work in progress.", "--draft"] }
```

### Checkout a PR locally

```json
{ "program": "gh", "args": ["pr", "checkout", "123"] }
```

### Merge a PR

```json
{ "program": "gh", "args": ["pr", "merge", "123", "--squash", "--delete-branch"] }
```

Merge strategies: `--merge`, `--squash`, `--rebase`.

### Close a PR without merging

```json
{ "program": "gh", "args": ["pr", "close", "123"] }
```

### List PR comments

```json
{ "program": "gh", "args": ["api", "repos/{owner}/{repo}/pulls/123/comments"] }
```

### Add a PR comment

```json
{ "program": "gh", "args": ["pr", "comment", "123", "--body", "Looks good, one suggestion on the error handling."] }
```

### Request reviewers

```json
{ "program": "gh", "args": ["pr", "edit", "123", "--add-reviewer", "username"] }
```

---

## GitHub CLI: Issues

### List open issues

```json
{ "program": "gh", "args": ["issue", "list", "--state", "open"] }
```

Filter by label:

```json
{ "program": "gh", "args": ["issue", "list", "--label", "bug", "--state", "open"] }
```

### View issue details

```json
{ "program": "gh", "args": ["issue", "view", "42"] }
```

### Create an issue

```json
{ "program": "gh", "args": ["issue", "create", "--title", "Bug: login fails on empty password", "--body", "## Steps to reproduce\n1. Go to login\n2. Leave password empty\n3. Click submit\n\n## Expected\nValidation error\n\n## Actual\n500 error"] }
```

With labels and assignee:

```json
{ "program": "gh", "args": ["issue", "create", "--title", "Bug: login fails", "--body", "Details here.", "--label", "bug", "--assignee", "username"] }
```

### Close an issue

```json
{ "program": "gh", "args": ["issue", "close", "42", "--reason", "completed"] }
```

### Add a comment to an issue

```json
{ "program": "gh", "args": ["issue", "comment", "42", "--body", "Fixed in PR #55."] }
```

---

## GitHub CLI: Repository & API

### View repo info

```json
{ "program": "gh", "args": ["repo", "view"] }
```

### List releases

```json
{ "program": "gh", "args": ["release", "list"] }
```

### Create a release

```json
{ "program": "gh", "args": ["release", "create", "v1.0.0", "--title", "v1.0.0", "--notes", "Initial release."] }
```

### Raw API calls

For endpoints not covered by `gh` subcommands, use `gh api`:

```json
{ "program": "gh", "args": ["api", "repos/{owner}/{repo}/actions/runs", "--jq", ".[0:5]"] }
```

List workflow runs:

```json
{ "program": "gh", "args": ["api", "repos/{owner}/{repo}/actions/runs", "--jq", ".workflow_runs[:5] | .[] | {id, status, conclusion, name}"] }
```

### Check CI status for current branch

```json
{ "program": "gh", "args": ["pr", "checks"] }
```

---

## Common Workflows

### Feature branch → PR

1. Create branch:
   `git checkout -b feature/my-feature`
2. Make changes, then stage:
   `git add -A`
3. Commit:
   `git commit -m "feat: description"`
4. Push:
   `git push -u origin feature/my-feature`
5. Create PR:
   `gh pr create --title "feat: description" --body "Details."`

### Sync feature branch with main

1. Fetch latest:
   `git fetch origin`
2. Rebase onto main:
   `git rebase origin/main`
3. Force push (if already pushed):
   `git push --force-with-lease`

Use `--force-with-lease` instead of `--force` to avoid overwriting others' work.

### Investigate and fix an issue

1. Read issue:
   `gh issue view 42`
2. Create fix branch:
   `git checkout -b fix/issue-42`
3. Make changes, commit, push, open PR:
   `gh pr create --title "fix: resolve issue #42" --body "Closes #42."`

### Review a PR

1. Checkout PR:
   `gh pr checkout 123`
2. View diff:
   `gh pr diff 123`
3. Run tests locally
4. Add review comment:
   `gh pr comment 123 --body "LGTM with one nit."`

---

## Safety Notes

- **Read-only** (safe for hand agents): `status`, `log`, `diff`, `show`, `blame`, `branch --list`, `tag --list`, `stash list`, `gh pr list`, `gh pr view`, `gh issue list`, `gh issue view`, `gh repo view`, `gh pr checks`
- **Mutating** (requires head/mind): `add`, `commit`, `push`, `merge`, `rebase`, `reset`, `stash push/pop/drop`, `tag -a`, `branch -d`, `gh pr create/merge/close/comment`, `gh issue create/close/comment`, `gh release create`
- **Destructive** (use with caution): `push --force`, `reset --hard`, `branch -D`, `clean -fd`. Prefer `--force-with-lease` over `--force`. Avoid `reset --hard` unless recovery from a known-bad state.
- Always check `git status` before committing to verify what will be included.
- Use `git diff --cached` to review staged changes before committing.
