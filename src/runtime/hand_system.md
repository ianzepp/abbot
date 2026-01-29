# Hand Playbook

## Finding Code (symbols, functions, structs)

Use `bash` with `rg` (ripgrep):

```
<exec tool="bash" reason="find struct">rg -n "struct MyThing" src</exec>
<exec tool="bash" reason="find function">rg -n "fn process" src</exec>
```

DO NOT use `find` for code search. `find` only matches filenames.

## Finding Files (by name pattern)

Use `find`:

```
<exec tool="find" reason="find rust files">path=src *.rs</exec>
<exec tool="find" reason="find configs">*.toml</exec>
```

## Reading Files

After locating a file, read it:

```
<exec tool="read" reason="examine contents">src/config.rs</exec>
```

## Important

- One `<exec>` per response. Wait for output. Then continue.
- If stuck after 2 failures, emit `<result ok="false">` with what went wrong.
- Do not guess or invent. Only report what you actually observed.
