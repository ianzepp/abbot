# TUI + Monitor Merge Plan

Merge `abbot-tui` and `abbot-monitor` into a single binary. Replace WireFrame
with raw Frame on WebSocket. Drop the UDS dependency from the merged client.
Exclude the Logs view (monitor's log browser against `/admin/logs`).

## Current State

### abbot-tui (8 files, ~1200 lines)
- Connects via **WebSocket** (`ws://addr/ws`)
- Receives simplified `WireFrame` on broadcast, `chat.*` messages for conversations
- Sends `chat.send`, `chat.cancel`, `farewell.request`
- Uses HTTP `/admin/logs` for replay on reconnect
- Views: multi-room chat with tabs, activity feed sidebar
- Binary: `abbot-tui`

### abbot-monitor (6 files, ~3500 lines)
- Connects via **UDS** (`~/.abbot/frames.sock`) for raw Frame stream
- Uses HTTP `/admin/config` for config editor, `/admin/logs` for log browser
- Views: Monitor (frame table + detail), Config (JSON editor), Logs (search + browse)
- Binary: `abbot-monitor`

### Daemon WebSocket (websocket.rs, ~960 lines)
- `WsOutMessage`: 12 variants (connected, pong, frame, frame.detail, chat.ack/delta/tool/done/error/status/mind, farewell, error)
- `WsInMessage`: 5 variants (ping, chat.send, chat.cancel, frame.detail, farewell.request)
- `WireFrame` + `simplify_frame()` + `summarize_frame()` — lossy projection of Frame
- `frame.detail` request/response — on-demand full Frame lookup by ID from SQLite
- Frame broadcast arm subscribes to `k.subscribe_frames()`, sends `WireFrame`

### Daemon UDS (frames_uds.rs, ~150 lines)
- Write-only: sends `Connected` + 100 recent frames + live broadcast
- Raw `Frame` (full data + trace), newline-delimited JSON
- 0600 socket permissions, loopback trust model

## Target State

One binary (`abbot-tui`) with three views:
1. **Chat** — current TUI chat (rooms, streaming, markdown)
2. **Monitor** — current monitor frame table (filtering, detail overlay, sparkline)
3. **Config** — current monitor config editor (sections, fields, dialogs)

Connected via WebSocket only. Raw Frame on the wire. UDS remains for
programmatic consumers but the TUI no longer uses it.

---

## Phase 1: Raw Frame on WebSocket

**Goal**: Replace WireFrame with the canonical Frame struct on the WS broadcast.
Delete the simplification layer. This unblocks the merge by giving both views
the same data.

### 1.1 Daemon: Send raw Frame on broadcast

**File**: `daemon/src/server/websocket.rs`

- Change `WsOutMessage::Frame(WireFrame)` to `WsOutMessage::Frame(Frame)` where
  `Frame` is imported from `crate::kernel::Frame`
- Delete `WireFrame` struct, `simplify_frame()`, `summarize_frame()`, `truncate()`
- In the frame broadcast select arm, send the raw frame directly:
  ```rust
  Ok(frame) => {
      // mind:thought interception stays here
      if out_tx.send(WsOutMessage::Frame(frame)).await.is_err() {
          break;
      }
  }
  ```
- In the catchup phase, send `stored.frame` directly (no simplify)
- Delete `WsInMessage::FrameDetail` variant and `handle_frame_detail()` +
  `read_frame_by_id()` — clients have full data already
- Delete the `WsOutMessage::FrameDetail` variant

### 1.2 TUI: Consume raw Frame

**File**: `tui/src/ws.rs`

- Replace `WireFrame` struct with a `Frame` struct matching the daemon's
  canonical shape (all fields, serde Deserialize):
  ```rust
  pub struct Frame {
      pub id: uuid::Uuid,
      pub ts: i64,
      pub op: String,           // deserialized as string, not enum
      pub name: Option<String>,
      pub parent_id: Option<uuid::Uuid>,
      pub actor: Option<String>,
      pub deadline_ms: Option<u64>,
      pub trace: Option<serde_json::Value>,
      pub data: Option<serde_json::Value>,
  }
  ```
- Update `WsOutMessage::Frame(Frame)` to use new struct
- Delete `WsOutMessage::FrameDetail` variant (no longer exists)
- `WsEvent::Frame(Frame)` already passes through — no change needed

### 1.3 Verify

- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Manual: connect TUI, verify chat still works, frames arrive in event stream

---

## Phase 2: Merge Crates

**Goal**: Absorb monitor's Monitor view and Config view into abbot-tui. Drop
the abbot-monitor crate.

### 2.1 Consolidate Theme

The two themes overlap heavily but monitor's has extra fields. Merge into one.

**File**: `tui/src/theme.rs`

Add fields from monitor's theme that TUI lacks:
- `panel_header_bg` (selected row background in tables)
- `panel_bg` (sidebar/dialog background)
- `border_blue` (used by monitor for `req` op color)
- `selection` (bullet color for selected rows)
- `error_fg` (error message color)

Monitor's `border_magenta` already added in prior commit. Monitor has
`status_bar_bg` (unused) — skip it.

### 2.2 Add View System

**File**: `tui/src/app.rs`

Add top-level view enum and state:
```rust
pub enum View {
    Chat,
    Monitor,
    Config,
}
```

Add to `App`:
```rust
pub view: View,                            // active view tab
pub monitor: MonitorState,                 // monitor view state
pub config_editor: ConfigEditorState,      // config view state
```

### 2.3 Port Monitor State

**New file**: `tui/src/monitor.rs`

Port from `monitor/src/main.rs` (the frame ingestion + state) and
`monitor/src/monitor.rs` (the rendering):

- `MonitorState` struct (replaces monitor's `App` fields):
  - `frames: VecDeque<FrameRecord>`
  - `pending: HashMap<Uuid, usize>`
  - `timeline: VecDeque<TimelineBucket>`
  - `syscall_ticker: VecDeque<String>`
  - `view_mode: ViewMode` (Frames/Needs/Tasks)
  - `paused: bool`
  - `selected: usize`
  - Counters: `need_count`, `task_count`, `tool_count`, `reply_count`, `tick_count`
  - `show_detail: bool`
  - `queued_count: usize`

- `push_frame()`, `advance_timeline()`, `update_timeline()`, `monitor_total()`
  become methods on `MonitorState`

- `draw_monitor()`, `draw_frames()`, `draw_detail()`, `draw_monitor_status()`
  ported from `monitor/src/monitor.rs`

- `FrameRecord`, `ViewMode`, `TimelineBucket`, `FrameCounts` types come along

The monitor currently uses its own `Frame` struct (deserialized from UDS JSON).
Replace with the shared `Frame` struct from `tui/src/ws.rs`.

### 2.4 Port Config Editor

**New file**: `tui/src/config.rs`

Port from `monitor/src/config.rs` (~1060 lines) wholesale:

- `ConfigEditorState`, `ConfigSection`, `ConfigField`, `ConfigDialog`,
  `FieldType`, `FieldValue`, `ConfigFocus`, `JsonKind` — all types
- `load_from_json()`, `to_json()`, `build_section_from_json()`,
  `validate_for_save()` — all logic
- `draw_config()`, `draw_sections_panel()`, `draw_fields_panel()`,
  `draw_dialog()`, `draw_save_confirm()`, `draw_config_status()` — all rendering
- Async tasks: `fetch_config()`, `save_config()` move into a helper module or
  inline in main.rs

Update theme references to use the merged `Theme`.

### 2.5 Port Shared Widgets

**New file**: `tui/src/widgets.rs`

Port from `monitor/src/widgets.rs` (~336 lines):

- `draw_header()`, `draw_subheader()`, `draw_statusline()`, `draw_top_nav()`,
  `draw_view_picker()`, `centered_rect()`, `op_color()`, `truncate()`,
  `split_at_char_boundary()`

These are used by both the monitor and config views. The TUI's existing room
rendering doesn't use them (it has its own layout), so there's no conflict.

`draw_top_nav()` needs updating — currently shows "[1] Monitor [2] Config [3] Logs".
Change to "[1] Chat [2] Monitor [3] Config". Drop the Logs tab entirely.

### 2.6 Wire Up the Event Loop

**File**: `tui/src/main.rs`

The main event loop gains:

**Frame ingestion**: Route `WsEvent::Frame` into `MonitorState::push_frame()`
instead of ignoring it:
```rust
WsEvent::Frame(frame) => {
    if app.monitor.paused {
        paused_queue.push_back(frame);
        // ...
    } else {
        app.monitor.push_frame(frame);
    }
}
```

**View switching**: Add keyboard handling for view tabs. In Normal mode:
- `1` → Chat, `2` → Monitor, `3` → Config
- `Ctrl+T` → view picker overlay (ported from monitor)

**Config async channels**: Add `config_tx`/`config_rx` channels and spawn
`fetch_config()` / `save_config()` tasks on view switch and Ctrl+R/Ctrl+S.

**Keyboard dispatch by view**: The current event handling assumes Chat view.
Wrap in a view match:
```rust
match app.view {
    View::Chat => { /* existing chat keyboard handling */ }
    View::Monitor => { /* monitor keys: a/n/t/p/j/k/Enter */ }
    View::Config => { /* config keys: section/field/dialog navigation */ }
}
```

Global keys (Ctrl+C, Ctrl+T, number tabs) handled before the view dispatch.

### 2.7 Update UI Layout

**File**: `tui/src/ui.rs`

The top-level `draw()` function dispatches by view:
```rust
pub fn draw(f: &mut Frame, app: &App) {
    match app.view {
        View::Chat => draw_chat(f, app),     // existing draw() logic
        View::Monitor => monitor::draw_monitor(f, app),
        View::Config => config::draw_config(f, app),
    }
}
```

The Chat view keeps its existing header (room tabs, clock, connection dot).
Monitor and Config views use the ported `draw_top_nav()` header instead.

### 2.8 Clean Up

- Remove `monitor/` crate directory
- Remove `monitor` from workspace `Cargo.toml` members
- Add any missing deps to `tui/Cargo.toml` (monitor needs `dirs`, `uuid` with
  serde — check if already present)
- Delete `WsOutMessage::FrameDetail` from daemon if not already done
- Update any CI references to `abbot-monitor`

---

## Phase 3: Polish

### 3.1 Unified Status Bar

All three views should show a consistent status bar at the bottom. Ported from
monitor's `draw_statusline()` widget:
- Left: view-specific info (room name / frame count / config section)
- Right: global shortcuts hint + connection indicator

### 3.2 Chat View Gets Top Nav

Currently the Chat view has its own tab bar (room tabs). Consider whether to:
- Keep the room-tab header for Chat and use `draw_top_nav()` for Monitor/Config
- Or put `draw_top_nav()` on all views and move room tabs to a sub-header

Recommendation: keep room tabs as-is for Chat. The view-level tab switching
(1/2/3) works the same in all views but the header rendering differs per view.

### 3.3 Config HTTP Client

Monitor uses `reqwest::Client` with 2s connect / 5s total timeout for admin
API calls. TUI already has `reqwest` for replay. Consolidate the HTTP client
construction into a shared helper (or just inline `admin_http_client()` in the
config module).

### 3.4 Drop UDS from the Merged TUI

The merged binary no longer needs UDS. Remove:
- `tokio-tungstenite` already present for WS — no new deps needed
- No `frames_sock` CLI arg or `ABBOT_FRAMES_SOCK` env var
- The UDS server in the daemon (`frames_uds.rs`) stays for external consumers

---

## File Inventory

### New Files (in tui/src/)
| File | Source | Lines (est) |
|------|--------|-------------|
| `monitor.rs` | monitor/src/main.rs (state) + monitor/src/monitor.rs (render) | ~500 |
| `config.rs` | monitor/src/config.rs | ~1060 |
| `widgets.rs` | monitor/src/widgets.rs | ~336 |

### Modified Files
| File | Changes |
|------|---------|
| `daemon/src/server/websocket.rs` | Delete WireFrame, simplify_frame, summarize_frame, frame.detail handler; send raw Frame |
| `tui/src/ws.rs` | Replace WireFrame with full Frame struct; delete FrameDetail variants |
| `tui/src/app.rs` | Add View enum, MonitorState, ConfigEditorState to App |
| `tui/src/main.rs` | View switching, frame ingestion, config async, keyboard dispatch per view |
| `tui/src/ui.rs` | Top-level draw dispatch by view |
| `tui/src/theme.rs` | Add panel_header_bg, panel_bg, border_blue, selection, error_fg |
| `tui/Cargo.toml` | Add `dirs`, verify `uuid` has serde feature |

### Deleted
| Path | Reason |
|------|--------|
| `monitor/` (entire crate) | Merged into tui |
| `daemon/src/server/websocket.rs` WireFrame + helpers | Replaced by raw Frame |

## Dependency Changes (tui/Cargo.toml)

Already present in both: `ratatui`, `crossterm`, `tokio`, `serde`, `serde_json`,
`chrono`, `clap`, `uuid`, `tui-input`, `reqwest`.

Add from monitor if missing:
- `dirs = "6"` (for default config paths, if config view needs it)

Remove nothing — monitor had `toml` but it was unused.

## Risk Notes

1. **Frame serialization mismatch**: The daemon's `Frame` uses `FrameOp` enum
   for `op`, which serializes as lowercase strings. The TUI deserializes `op`
   as `String`. This already works — the UDS monitor does the same thing and
   it's fine. Just don't try to deserialize into `FrameOp` on the client side.

2. **Large frames on WS**: Some frames carry substantial `data` payloads (LLM
   responses, tool output). The UDS already handles this volume. The WS writer
   has a 256-slot channel buffer. Monitor caps at 1000 frames in memory. Should
   be fine for loopback.

3. **Config editor references `/admin/config`**: This HTTP endpoint must exist
   on the daemon. It already does — no change needed.

4. **Keyboard conflict**: Chat's Insert mode consumes all character keys.
   View-switching numbers (1/2/3) only work in Normal mode. This is fine —
   Monitor and Config views don't have an Insert mode, so numbers work directly.
   Document: press Esc first to switch views from Chat.
