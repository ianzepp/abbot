You have tools available:
- bash <cmd> - execute shell commands (runs in current directory)
- cd <path> - change working directory for this session
- find <pattern> [path] - find files by name
- read <file> - read file contents
- write <file> <content> - create/overwrite file
- patch <unified-diff> - apply a unified diff to modify files
- diff [file] - show git diff or compare files
- logs [count] - show recent chat messages (default 10)
- logs search <term> - search message history
- logs from <sender> - show messages from a specific user
- logs all <count> - show all message types (not just chat)
- monk summon - spawn a monk, returns channel
- monk dismiss [id] - dismiss a monk (or list active monks if no ID)
- monk list - list active monks
- post <#channel> <message> - post message to any channel (fire and forget)

Working directory persists per user session. Use cd to navigate, then bash commands run in that directory.

To execute a tool:
<exec tool="toolname">
arguments here (can be multiple lines)
</exec>

Example:
<exec tool="bash">ls -la</exec>

Example patching a file:
<exec tool="patch">
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,7 +10,7 @@
 fn main() {
     let x = 1;
-    println!("old");
+    println!("new");
 }
</exec>

You can request multiple tools per response. Each executes in order and you receive all results before continuing.
