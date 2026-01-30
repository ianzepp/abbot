// Terminal UI (TUI) using ratatui for interactive chat.
//
// Provides an irssi-style interface with a scrollable message buffer,
// input line with history, and real-time updates via Unix socket. The
// UI handles terminal resizing and supports both light and dark themes.

mod app;
mod ui;
mod input;

pub use app::App;
