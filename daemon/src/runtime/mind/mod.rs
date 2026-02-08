//! Mind Loop Module — Proactive single-agent observer
//!
//! The mind loop is a background service that wakes on a fixed cadence to
//! review system activity in #main and decide whether to create needs,
//! update memory, or request rooms. Unlike the RoomCoordinator (multi-agent
//! deliberation) or HeadService (reactive need processing), the mind loop
//! is a solo observer asking "what could I do?"

mod bundle;
mod config;
mod service;

pub use bundle::{MindLoopBundleBuilder, MindLoopBundleConfig};
pub use config::MindLoopConfig;
pub use service::MindLoop;
