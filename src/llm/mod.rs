mod backend;
mod client;
mod openai_compat;

pub use backend::{Backend, resolve_model};
pub use client::LlmClient;
