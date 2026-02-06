mod chat;
mod chaos;
mod chaos_list;

pub use chat::LlmChat;
pub use chaos::LlmChaos;
pub use chaos_list::LlmChaosList;

use regex::Regex;

use crate::kernel::KernelError;
use crate::runtime::{HandConfig, HeadConfig, RoomConfig};

pub(crate) fn cfg_for_actor(actor: &str) -> Result<crate::runtime::Config, KernelError> {
    let a = actor.trim();
    let cfg = if a.starts_with("head/") {
        HeadConfig::from_config().llm
    } else if a.starts_with("hand/") {
        HandConfig::from_config().llm
    } else if a.starts_with("mind/") {
        RoomConfig::from_config().llm
    } else {
        return Err(KernelError::invalid_args(
            "llm:chat requires actor prefix head/*, hand/*, or mind/*",
        ));
    };

    if !cfg.enabled {
        return Err(KernelError::invalid_args(format!(
            "LLM not configured for actor '{actor}'",
        )));
    }
    Ok(cfg)
}

pub(crate) fn parse_llm_content(content: &str) -> (Option<String>, Option<String>) {
    let thinking_re = Regex::new(r"<thinking>([\s\S]*?)</thinking>").unwrap();

    let mut thinking_parts = Vec::new();
    let mut visible_content = content.to_string();

    for cap in thinking_re.captures_iter(content) {
        thinking_parts.push(cap[1].to_string());
        visible_content = visible_content.replace(&cap[0], "");
    }

    let thinking = if thinking_parts.is_empty() {
        None
    } else {
        Some(thinking_parts.join("\n"))
    };

    let visible = visible_content.trim();
    let visible = if visible.is_empty() {
        None
    } else {
        Some(visible.to_string())
    };

    (thinking, visible)
}

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(LlmChat::new()));
    dispatcher.register(Arc::new(LlmChaos::new()));
    dispatcher.register(Arc::new(LlmChaosList::new()));
}
