# Abbot

Persistent AI daemon with Mind/Head/Hand architecture. Rust backend, React frontend.

## Structure

- `src/runtime/` - Mind, Head, Hand services and conclave deliberation
- `src/agent_tools.rs` - Tool definitions
- `src/history/` - SQLite storage
- `src/server/` - HTTP API and WebSocket
- `web/` - React frontend

## Key Patterns

- Mind creates needs, Head converts to goals, Hand executes tools
- Conclave: 3 personas deliberate to consensus (2/3 vote)
- Memory: LTM (strategic) flows to heads, STM (tactical) flows to hands, Self (identity) defines who we are
- All file ops sandboxed to workspace

## Building

```bash
cargo build
cd web && npm run build
```

## Running

```bash
RUST_LOG=info ./target/debug/abbot run
```
