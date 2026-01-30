// Unix socket transport for local TUI and CLI clients.
//
// Provides a lightweight local-only communication channel that doesn't
// require network configuration. The TUI uses this for real-time bidirectional
// communication with the server. Messages use length-prefixed JSON framing.

mod wire;
mod listener;

pub use wire::WireMessage;
pub use listener::SocketListener;
