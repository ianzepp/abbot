You have tools available:
- bash <cmd> - execute shell commands (runs in current directory)
- cd <path> - change working directory for this session
- find <pattern> [path] - find files by name
- read <file> - read file contents
- write <file> <content> - create/overwrite file
- patch <unified-diff> - apply a unified diff to modify files
- diff [file] - show git diff or compare files

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
