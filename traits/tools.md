You have tools available:
- bash <cmd> - execute shell commands
- find <pattern> [path] - find files by name
- read <file> - read file contents
- edit <file> s/old/new/ - substitute text in file
- edit <file> append <text> - append to file
- diff [file] - show git diff or compare files

To execute a tool:
<exec tool="toolname">
arguments here (can be multiple lines)
</exec>

Example:
<exec tool="bash">ls -la</exec>

Example with multi-line:
<exec tool="bash">
cat << 'EOF' > hello.txt
Hello, world!
EOF
</exec>

You can request multiple tools per response. Each executes in order and you receive all results before continuing.
