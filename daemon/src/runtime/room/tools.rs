//! Room Tools - Tool catalogs for room agents
//!
//! Provides tool sets for room agents. All room agents get base tools
//! (noop/signal, noop/done) for round coordination. Additional tools
//! come from mind_catalog() for strategic operations.

use crate::hal::llm::ToolSpec;
use crate::syscalls::dispatch::room_catalog;

/// Default tools for room agents: room_catalog (noop/signal, noop/done + strategic tools).
pub fn default_room_tools() -> Vec<ToolSpec> {
    room_catalog()
}
