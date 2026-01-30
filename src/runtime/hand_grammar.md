# Hand Response Format

## Structure

Each response: ONE exec block OR ONE result block. Not both. Not zero.

Text outside blocks is for thinking (ignored).

## Exec Block

```exec tool [key=value ...]
content
```

## Result Block

```result ok
summary of what was accomplished
```

```result fail
what went wrong
```

## Tools

### bash
Run shell commands. Use `rg` for code search, `ls` for listing, etc.
```exec bash
rg -n "struct Config" src
```

### read
Read file contents. Optional: offset, limit.
```exec read
src/config.rs
```

```exec read offset=100 limit=50
src/config.rs
```

### write
Create or overwrite a file. Path in header, content in body.
```exec write path=src/new_file.rs
use std::io;

fn main() {
    println!("hello");
}
```

### edit
Modify part of a file. Path in header, OLD/NEW block in body.
```exec edit path=src/lib.rs
<<<<<<< OLD
let naem = "test";
=======
let name = "test";
>>>>>>> NEW
```

### find
Find files by name pattern. Optional: path.
```exec find
*.toml
```

```exec find path=src
*.rs
```

### diff
Compare files or show git changes.
```exec diff
file1.txt file2.txt
```

```exec diff
git
```

### patch
Apply a unified diff. Path in header.
```exec patch path=src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,3 @@
-old line
+new line
```

### cd
Change working directory.
```exec cd
src/runtime
```

## Rules

1. One exec per response. Wait for output. Then continue.
2. Do not combine exec and result in the same response.
3. When done, emit a result block.
4. When stuck, emit result fail with what blocked you.
