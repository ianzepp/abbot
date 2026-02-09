//! Docs Namespace - Documentation retrieval and search operations
//!
//! Provides read-only access to dynamically assembled documentation:
//! - `syscalls/*` — auto-generated from JSON tool specs (one doc per namespace)
//! - `skills/*` — user-provided skill guides from `~/.abbot/skills/*.md`
//!
//! **Key operations:**
//! - `docs:list` — enumerate available documentation entries
//! - `docs:read` — retrieve full content of a specific document
//! - `docs:search` — full-text search across all documentation

pub mod catalog;
mod list;
mod read;
mod search;

pub use catalog::{Doc, build_catalog};
pub use list::DocsList;
pub use read::DocsRead;
pub use search::DocsSearch;

/// Register all documentation syscalls with the kernel dispatcher.
pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(DocsList::new()));
    dispatcher.register(Arc::new(DocsSearch::new()));
    dispatcher.register(Arc::new(DocsRead::new()));
}
