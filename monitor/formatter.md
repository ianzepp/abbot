# TUI Formatting & Documentation Guide

Lightweight documentation conventions for the TUI crate. The daemon's full formatter
(see `skills/formatter.md`) is designed for kernel internals with security boundaries,
concurrency models, and syscall dispatch. The TUI is declarative UI code — apply
documentation where it earns its keep.

## File-Level Documentation

One-liner `//!` doc at the top of each module. Skip multi-section headers.

```rust
//! Chat view — message display, compose input, and scope picker.
```

Add a second line only when the module has non-obvious responsibilities:

```rust
//! Config editor — section/field/dialog navigation and validation.
//!
//! Manages a three-level focus hierarchy (section → field → dialog) with
//! async config loading and saving via the daemon admin API.
```

## Section Dividers

Use dividers only in larger files (300+ lines) with genuinely distinct groups.
Keep the header short — no multi-line explanation blocks unless the grouping
is surprising.

```rust
// =============================================================================
// DRAWING
// =============================================================================
```

Typical sections for a view module:
- `TYPES` — view-specific state structs and enums
- `DRAWING` — `draw_*` functions
- `EVENT HANDLING` — keyboard/channel handlers
- `ASYNC` — spawned tasks, channel senders

## When to Write Doc Comments

**Do document:**
- Public structs/enums shared across modules (e.g., `Theme`, `View`, `ChatMode`)
- Non-obvious state fields (why a field exists, not what type it is)
- Complex rendering logic where layout math isn't self-evident
- Event handling branches that encode UX decisions

**Skip documentation for:**
- `draw_*` functions with obvious names (`draw_header`, `draw_statusline`)
- Simple enum variants that match their view name (`View::Chat`, `View::Monitor`)
- Struct fields where the name + type say everything (`selected: usize`, `dark_mode: bool`)
- One-off helper functions called from a single place

## Inline Comments

Focus on UX rationale — why a layout choice was made, why an interaction
works a certain way:

```rust
// WHY: Reserve 10 chars for nick column so messages align across senders
const NICK_WIDTH: usize = 10;

// WHY: 100ms tick balances responsiveness with CPU usage on idle terminals
let timeout = Duration::from_millis(100);
```

Skip comments that restate the code:

```rust
// BAD: "Split the layout into two chunks"
// GOOD: "WHY: sidebar needs fixed width to prevent re-flow on content changes"
```

## State & Event Patterns

Document non-obvious state transitions and channel flows:

```rust
/// Chat input modes.
///
/// Transitions: ScopePicker → Normal → Insert (on 'i') → Normal (on Esc).
/// ScopePicker only appears on initial view entry.
enum ChatMode {
    ScopePicker,
    Normal,
    Insert,
}
```

For async channel events, a brief comment at the enum or handler is enough:

```rust
/// Events received from the daemon WebSocket frame stream.
enum WsEvent {
    Frame(FrameRecord),
    Disconnected,
}
```

## What NOT to Apply from the Daemon Formatter

These daemon conventions don't fit TUI code — skip them here:

- **ARCHITECTURE OVERVIEW / DESIGN PHILOSOPHY** headers
- **SECURITY MODEL / CONCURRENCY** sections
- **Phase markers** inside render functions
- **TRADE-OFF / INVARIANT** formal markers on types
- **Error constructor documentation** (TUI doesn't define domain error types)

## Checklist

When writing or refactoring a TUI module:

- [ ] File has a one-liner `//!` doc
- [ ] Larger files use section dividers for distinct groups
- [ ] Shared types have brief doc comments
- [ ] Non-obvious layout/UX decisions have WHY comments
- [ ] State transition logic is documented where surprising
- [ ] No over-documentation of obvious rendering code
