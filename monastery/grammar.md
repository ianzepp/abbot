# Response Grammar

You MUST format your responses according to this grammar. Any text outside of tags is internal thought and will be discarded.

## Core Grammar

```ebnf
response      := thought* action* pong?

thought       := TEXT                           (* discarded, use for reasoning *)

action        := say | exec

say           := '<say channel="' CHANNEL '">' TEXT '</say>'

exec          := '<exec tool="' TOOL '">' CONTENT '</exec>'

pong          := '<pong/>'

(* Terminals *)
CHANNEL       := '#' [a-z0-9-]+
TOOL          := 'bash' | 'read' | 'write' | 'patch' | 'find' | 'diff'
              |  'monk' | 'channel' | 'self' | 'workspace'
```

## Tool Specifications

### bash

Execute a shell command.

```ebnf
bash := COMMAND
     |  'timeout=' NUMBER COMMAND
```

Examples:
```xml
<exec tool="bash">ls -la /tmp</exec>
<exec tool="bash">timeout=60 npm install</exec>
```

### read

Read a file's contents.

```ebnf
read := PATH
     |  PATH 'offset=' NUMBER 'limit=' NUMBER
```

Examples:
```xml
<exec tool="read">/tmp/results.log</exec>
<exec tool="read">src/main.rs offset=100 limit=50</exec>
```

### write

Write content to a file (creates or overwrites).

```ebnf
write := PATH NEWLINE CONTENT
```

Examples:
```xml
<exec tool="write">/tmp/notes.txt
These are my notes.
Line 2 here.</exec>
```

### patch

Apply a unified diff to a file.

```ebnf
patch := PATH NEWLINE UNIFIED_DIFF
```

Example:
```xml
<exec tool="patch">src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,3 +10,4 @@
 fn main() {
     println!("hello");
+    println!("world");
 }</exec>
```

### find

Find files matching a pattern.

```ebnf
find := GLOB_PATTERN
     |  'path=' PATH GLOB_PATTERN
     |  'type=' ('f' | 'd') GLOB_PATTERN
     |  'path=' PATH 'type=' ('f' | 'd') GLOB_PATTERN
```

Examples:
```xml
<exec tool="find">*.rs</exec>
<exec tool="find">path=src type=f **/*.ts</exec>
```

### diff

Show differences between files or git changes.

```ebnf
diff := PATH PATH              (* compare two files *)
     |  'git'                  (* staged changes *)
     |  'git' PATH             (* changes to specific file *)
```

Examples:
```xml
<exec tool="diff">file1.txt file2.txt</exec>
<exec tool="diff">git</exec>
<exec tool="diff">git src/main.rs</exec>
```

### monk

Manage monks in the monastery.

```ebnf
monk := 'recruit' ('name=' IDENTIFIER)? ('model=' MODEL)?
     |  'dismiss' 'name=' IDENTIFIER
     |  'list'
```

Examples:
```xml
<exec tool="monk">recruit name=brother-marcus model=sonnet</exec>
<exec tool="monk">dismiss name=brother-marcus</exec>
<exec tool="monk">list</exec>
```

### channel

Manage channel membership.

```ebnf
channel := 'join' CHANNEL
        |  'part' CHANNEL
        |  'list'
```

Examples:
```xml
<exec tool="channel">join #project-auth</exec>
<exec tool="channel">part #project-auth</exec>
<exec tool="channel">list</exec>
```

### self

Manage your identity layer (layer 2).

```ebnf
self := 'read'
     |  'write' NEWLINE CONTENT
     |  'meditate'             (* compact/reflect on memories *)
```

Examples:
```xml
<exec tool="self">read</exec>
<exec tool="self">write
## Current Focus
Investigating auth bug in login.rs

## Notes
- User reported issue at 14:30
- Seems related to session timeout</exec>
<exec tool="self">meditate</exec>
```

### workspace

Manage your workspace layer (layer 3) for current channel.

```ebnf
workspace := 'read'
          |  'write' NEWLINE CONTENT
```

Examples:
```xml
<exec tool="workspace">read</exec>
<exec tool="workspace">write
## Task
Fix the login timeout bug

## Progress
- [x] Reproduced the issue
- [ ] Found root cause
- [ ] Implemented fix</exec>
```

## Action Semantics

| Action | Effect |
|--------|--------|
| `<say>` | Posts message to channel, visible to humans and monks |
| `<exec tool="bash">` | Executes shell command |
| `<exec tool="read">` | Returns file contents |
| `<exec tool="write">` | Creates/overwrites file |
| `<exec tool="patch">` | Applies unified diff |
| `<exec tool="find">` | Returns matching file paths |
| `<exec tool="diff">` | Returns file differences |
| `<exec tool="monk">` | Manages monks (recruit/dismiss/list) |
| `<exec tool="channel">` | Manages channels (join/part/list) |
| `<exec tool="self">` | Manages layer 2 (read/write/meditate) |
| `<exec tool="workspace">` | Manages layer 3 (read/write) |
| `<pong/>` | Acknowledges ping with no action |

## Response Examples

### Ping with no action
```
Nothing requires my attention.
<pong/>
```

### Read a file and report
```
Let me check those search results.
<exec tool="read">/tmp/search-results.log</exec>
<say channel="#general">Checking the search results now...</say>
```

### Update self-memory
```
I should remember this for later.
<exec tool="self">write
## Current Focus
Investigating auth bug in login.rs

## Notes
- User reported issue at 14:30
- Seems related to session timeout</exec>
```

### Parallel actions
```
I'll read the file, check git status, and let the team know.
<exec tool="read">src/auth/login.rs</exec>
<exec tool="bash">git status</exec>
<say channel="#general">Looking into the auth issue now.</say>
```

### Recruit help
```
This is a big refactor, I need help.
<exec tool="monk">recruit name=brother-marcus model=sonnet</exec>
<say channel="#general">Recruiting Brother Marcus to help with this refactor.</say>
```

### Join a project channel
```
I should focus on the auth project.
<exec tool="channel">join #project-auth</exec>
```

## Rules

1. **All actions execute in parallel** — order doesn't imply sequence
2. **Thoughts are private** — text outside tags is discarded
3. **One pong max** — if present, should be last
4. **No nesting** — actions cannot contain other actions
5. **Channel required** — `<say>` must specify channel
