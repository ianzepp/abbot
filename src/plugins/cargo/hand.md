Use `cargo` for Rust project operations.

- Prefer the narrowest command that answers the question (e.g. `cargo test -p <crate>` instead of all tests).
- Use `cargo fmt` / `cargo clippy` when requested or when needed to satisfy CI.
- If a command fails, rerun with the smallest reproduction and include key stderr.
