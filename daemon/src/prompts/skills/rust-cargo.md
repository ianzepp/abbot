---
name: rust-cargo
description: Rust development with cargo, clippy, and rustfmt via exec:run
category: development
requires:
  - exec
---

# Rust, Cargo, Clippy & Rustfmt

Use `exec:run` with `program: "cargo"` for most Rust development tasks. Use `program: "rustc"` or `program: "rustfmt"` only when a direct compiler/formatter invocation is needed outside of cargo.

## Calling Convention

```json
{ "program": "cargo", "args": ["test", "--lib", "-p", "my-crate"] }
{ "program": "cargo", "args": ["clippy", "--", "-D", "warnings"] }
```

Use `cwd` to target a specific VFS-mounted project when the workspace has multiple roots:

```json
{ "program": "cargo", "args": ["check"], "cwd": "/projects/backend" }
```

## Read-Only vs Mutating

Hand agents may only run **read-only** commands (check, test, clippy, fmt --check, tree, metadata). Mutating commands (build with side effects, add/remove dependencies, publish) require **head or mind** role.

---

## Checking & Building

### Type-check without producing binaries

```json
{ "program": "cargo", "args": ["check"] }
```

Check a specific package in a workspace:

```json
{ "program": "cargo", "args": ["check", "-p", "my-crate"] }
```

### Build

Debug build:

```json
{ "program": "cargo", "args": ["build"] }
```

Release build:

```json
{ "program": "cargo", "args": ["build", "--release"] }
```

Build a specific binary:

```json
{ "program": "cargo", "args": ["build", "--bin", "my-binary"] }
```

Build a specific package:

```json
{ "program": "cargo", "args": ["build", "-p", "my-crate"] }
```

### Run a binary

```json
{ "program": "cargo", "args": ["run"] }
```

With arguments passed to the binary:

```json
{ "program": "cargo", "args": ["run", "--bin", "my-binary", "--", "--port", "8080"] }
```

---

## Testing

### Run all tests

```json
{ "program": "cargo", "args": ["test"] }
```

### Run only library tests (skip integration tests and doc tests)

```json
{ "program": "cargo", "args": ["test", "--lib"] }
```

### Run tests for a specific package

```json
{ "program": "cargo", "args": ["test", "-p", "my-crate"] }
```

### Run a specific test by name

```json
{ "program": "cargo", "args": ["test", "test_name_substring"] }
```

### Run tests in a specific module

```json
{ "program": "cargo", "args": ["test", "module::submodule"] }
```

### Run a specific integration test file

```json
{ "program": "cargo", "args": ["test", "--test", "integration_test_name"] }
```

### Run doc tests only

```json
{ "program": "cargo", "args": ["test", "--doc"] }
```

### Run tests with output visible (nocapture)

```json
{ "program": "cargo", "args": ["test", "--", "--nocapture"] }
```

### Run tests with limited parallelism

```json
{ "program": "cargo", "args": ["test", "--", "--test-threads=1"] }
```

### Run ignored tests

```json
{ "program": "cargo", "args": ["test", "--", "--ignored"] }
```

### Combine package + filter + flags

```json
{ "program": "cargo", "args": ["test", "--lib", "-p", "my-crate", "test_name", "--", "--nocapture"] }
```

Note: arguments before `--` go to cargo, arguments after `--` go to the test harness.

---

## Linting with Clippy

### Run clippy

```json
{ "program": "cargo", "args": ["clippy"] }
```

### Treat all warnings as errors

```json
{ "program": "cargo", "args": ["clippy", "--", "-D", "warnings"] }
```

### Clippy on a specific package

```json
{ "program": "cargo", "args": ["clippy", "-p", "my-crate", "--", "-D", "warnings"] }
```

### Clippy on all targets (tests, benches, examples)

```json
{ "program": "cargo", "args": ["clippy", "--all-targets", "--", "-D", "warnings"] }
```

### Clippy on entire workspace

```json
{ "program": "cargo", "args": ["clippy", "--workspace", "--", "-D", "warnings"] }
```

### Allow a specific lint

```json
{ "program": "cargo", "args": ["clippy", "--", "-D", "warnings", "-A", "clippy::too_many_arguments"] }
```

### Fix clippy warnings automatically

```json
{ "program": "cargo", "args": ["clippy", "--fix", "--allow-dirty"] }
```

---

## Formatting

### Check formatting (no changes)

```json
{ "program": "cargo", "args": ["fmt", "--check"] }
```

### Check formatting for a specific package

```json
{ "program": "cargo", "args": ["fmt", "-p", "my-crate", "--check"] }
```

### Apply formatting

```json
{ "program": "cargo", "args": ["fmt"] }
```

### Format a specific package

```json
{ "program": "cargo", "args": ["fmt", "-p", "my-crate"] }
```

### Format with rustfmt directly (single file)

```json
{ "program": "rustfmt", "args": ["src/main.rs"] }
```

### Check rustfmt diff without writing

```json
{ "program": "rustfmt", "args": ["--check", "src/main.rs"] }
```

---

## Dependencies

### Add a dependency

```json
{ "program": "cargo", "args": ["add", "serde"] }
```

### Add with features

```json
{ "program": "cargo", "args": ["add", "serde", "--features", "derive"] }
```

### Add a dev dependency

```json
{ "program": "cargo", "args": ["add", "tokio-test", "--dev"] }
```

### Add a build dependency

```json
{ "program": "cargo", "args": ["add", "cc", "--build"] }
```

### Add to a specific workspace package

```json
{ "program": "cargo", "args": ["add", "serde", "-p", "my-crate"] }
```

### Remove a dependency

```json
{ "program": "cargo", "args": ["remove", "old-crate"] }
```

### Update dependencies

Update all:

```json
{ "program": "cargo", "args": ["update"] }
```

Update a specific crate:

```json
{ "program": "cargo", "args": ["update", "serde"] }
```

---

## Dependency Inspection

### Show dependency tree

```json
{ "program": "cargo", "args": ["tree"] }
```

### Tree for a specific package

```json
{ "program": "cargo", "args": ["tree", "-p", "my-crate"] }
```

### Find why a crate is included (inverted tree)

```json
{ "program": "cargo", "args": ["tree", "-i", "openssl-sys"] }
```

### Tree with duplicates highlighted

```json
{ "program": "cargo", "args": ["tree", "--duplicates"] }
```

### Show features enabled for a dependency

```json
{ "program": "cargo", "args": ["tree", "-e", "features", "-i", "tokio"] }
```

---

## Workspace Operations

### Check the entire workspace

```json
{ "program": "cargo", "args": ["check", "--workspace"] }
```

### Test the entire workspace

```json
{ "program": "cargo", "args": ["test", "--workspace"] }
```

### List workspace members

```json
{ "program": "cargo", "args": ["metadata", "--format-version=1", "--no-deps"] }
```

The `workspace_members` field in the JSON output lists all packages. Pipe through `jq` if available:

```json
{ "program": "cargo", "args": ["metadata", "--format-version=1", "--no-deps"], "stdin": "" }
```

Then parse the `workspace_members` array from the JSON stdout.

---

## Documentation

### Build docs

```json
{ "program": "cargo", "args": ["doc", "--no-deps"] }
```

### Build docs for a specific package

```json
{ "program": "cargo", "args": ["doc", "-p", "my-crate", "--no-deps"] }
```

### Build docs and open in browser

```json
{ "program": "cargo", "args": ["doc", "--no-deps", "--open"] }
```

---

## Compiler Diagnostics

### Get expanded macros (requires nightly or cargo-expand)

```json
{ "program": "cargo", "args": ["expand", "--lib"] }
```

### Explain a compiler error code

```json
{ "program": "rustc", "args": ["--explain", "E0308"] }
```

### Show the edition and toolchain

```json
{ "program": "rustc", "args": ["--version", "--verbose"] }
```

---

## Common Workflows

### Pre-commit check (format + clippy + test)

1. Format:
   `cargo fmt`
2. Lint:
   `cargo clippy -- -D warnings`
3. Test:
   `cargo test`

### Diagnose a build failure

1. Run check to get errors without full build:
   `cargo check 2>&1`
2. Read the error output — look for file paths and line numbers.
3. If an error code is unclear:
   `rustc --explain E0XXX`

### Diagnose a test failure

1. Run the failing test with output:
   `cargo test failing_test_name -- --nocapture`
2. Run with single thread for deterministic ordering:
   `cargo test failing_test_name -- --nocapture --test-threads=1`

### Add a new dependency and verify

1. Add:
   `cargo add new-crate --features feature1`
2. Check it compiles:
   `cargo check`
3. Run tests:
   `cargo test`

### Investigate a clippy lint

1. Run clippy:
   `cargo clippy -- -D warnings`
2. Read the lint name from output (e.g., `clippy::needless_borrow`).
3. Fix, or suppress if justified with `#[allow(clippy::lint_name)]` on the item.

---

## Safety Notes

- **Read-only** (safe for hand agents): `check`, `test`, `clippy`, `fmt --check`, `tree`, `metadata`, `doc`, `rustc --explain`, `rustc --version`
- **Mutating** (requires head/mind): `build`, `run`, `fmt` (applies changes), `clippy --fix`, `add`, `remove`, `update`, `publish`
- `cargo test` executes arbitrary code in `#[test]` functions — treat it as read-only for linting purposes but be aware it runs project code.
- Always use `--no-deps` with `cargo doc` unless you specifically need dependency docs (avoids long builds).
- Use `-p package-name` to scope operations in a workspace — avoids rebuilding everything.
- Arguments after `--` are passed to the underlying tool (test harness, clippy/rustc, the binary itself), not to cargo.
