//! Provider cache — shared types and helpers for reading cached provider model lists.
//!
//! Provider model lists are cached on disk at `~/.abbot/providers/{name}.json` after
//! being fetched from each provider's API. This module provides the deserialization
//! types and utility functions used by both the admin API and safe mode recovery.

use std::path::PathBuf;

use serde::Deserialize;

use crate::runtime::app_config;

// =============================================================================
// TYPES
// =============================================================================

/// A cached provider model list, as stored on disk.
#[derive(Debug, Deserialize)]
pub struct ProviderCache {
    pub provider: String,
    pub fetched_at: String,
    pub models: Vec<CachedModel>,
}

/// A single model entry from a provider's cached model list.
#[derive(Debug, Clone, Deserialize)]
pub struct CachedModel {
    pub id: String,
    pub name: Option<String>,
    pub context_window: Option<u64>,
    #[serde(default)]
    pub input_cost: Option<f64>,
    #[serde(default)]
    pub output_cost: Option<f64>,
}

// =============================================================================
// LOADING
// =============================================================================

/// Load the cached model list for a single provider from `~/.abbot/providers/{name}.json`.
pub fn load_provider_cache(provider: &str) -> Option<ProviderCache> {
    let dir = app_config::providers_dir()?;
    let path = dir.join(format!("{provider}.json"));
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Load all cached provider model lists from `~/.abbot/providers/*.json`.
pub fn load_all_provider_caches() -> Vec<ProviderCache> {
    let Some(dir) = app_config::providers_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut caches = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json")
            && let Ok(raw) = std::fs::read_to_string(&path)
            && let Ok(cache) = serde_json::from_str::<ProviderCache>(&raw)
        {
            caches.push(cache);
        }
    }
    caches
}

/// Return the providers directory path.
pub fn providers_dir() -> Option<PathBuf> {
    app_config::providers_dir()
}

// =============================================================================
// FILTERING & SORTING
// =============================================================================

/// Heuristic: returns true if the model ID likely refers to a chat/completion model
/// (as opposed to embedding, TTS, image generation, moderation, etc.).
pub fn is_likely_chat_model(model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();

    // Exclude known non-chat model families by substring.
    const EXCLUDE_PATTERNS: &[&str] = &[
        "embed",
        "tts",
        "whisper",
        "dall-e",
        "dalle",
        "image",
        "moderation",
        "audio",
        "realtime",
        "search",
        "transcription",
        "speech",
        "vision-preview", // old preview-only vision models
        "text-embedding",
        "babbage",
        "davinci",
        "curie",
        "ada",
    ];

    for pattern in EXCLUDE_PATTERNS {
        if id.contains(pattern) {
            return false;
        }
    }

    true
}

/// Sort models for probing: cheap/small models first, expensive/large models last.
/// This minimizes cost when probing for a healthy model.
pub fn sort_models_for_probe(models: &mut [CachedModel]) {
    models.sort_by(|a, b| {
        let pa = probe_priority(&a.id);
        let pb = probe_priority(&b.id);
        pa.cmp(&pb)
    });
}

/// Lower = higher priority for probing (cheaper/smaller models first).
fn probe_priority(model_id: &str) -> u8 {
    let id = model_id.to_ascii_lowercase();

    // Tier 0: ultra-cheap probe models
    if id.contains("mini") || id.contains("haiku") || id.contains("flash") || id.contains("small") {
        return 0;
    }

    // Tier 1: mid-range models
    if id.contains("sonnet") || id.contains("4.1") || id.contains("gpt-4o") {
        return 1;
    }

    // Tier 2: capable but pricier
    if id.contains("gpt-4") || id.contains("claude-3") || id.contains("gemini") {
        return 2;
    }

    // Tier 3: expensive/large reasoning models — probe last
    if id.contains("opus")
        || id.contains("o1")
        || id.contains("o3")
        || id.contains("o4")
        || id.contains("deepseek")
    {
        return 3;
    }

    // Default: mid-range
    2
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_model_detection() {
        // Should be chat models
        assert!(is_likely_chat_model("gpt-4"));
        assert!(is_likely_chat_model("gpt-4o-mini"));
        assert!(is_likely_chat_model("claude-haiku-4-20250514"));
        assert!(is_likely_chat_model("claude-sonnet-4-20250514"));
        assert!(is_likely_chat_model("llama3"));
        assert!(is_likely_chat_model("openai/gpt-4.1-mini"));
        assert!(is_likely_chat_model("gemini-2.0-flash"));

        // Should NOT be chat models
        assert!(!is_likely_chat_model("text-embedding-3-large"));
        assert!(!is_likely_chat_model("tts-1"));
        assert!(!is_likely_chat_model("tts-1-hd"));
        assert!(!is_likely_chat_model("whisper-1"));
        assert!(!is_likely_chat_model("dall-e-3"));
        assert!(!is_likely_chat_model("text-moderation-007"));
        assert!(!is_likely_chat_model("text-embedding-ada-002"));
    }

    #[test]
    fn probe_priority_ordering() {
        let mut models = vec![
            CachedModel {
                id: "claude-opus-4".into(),
                name: None,
                context_window: None,
                input_cost: None,
                output_cost: None,
            },
            CachedModel {
                id: "gpt-4.1-mini".into(),
                name: None,
                context_window: None,
                input_cost: None,
                output_cost: None,
            },
            CachedModel {
                id: "claude-sonnet-4".into(),
                name: None,
                context_window: None,
                input_cost: None,
                output_cost: None,
            },
            CachedModel {
                id: "o3-pro".into(),
                name: None,
                context_window: None,
                input_cost: None,
                output_cost: None,
            },
        ];

        sort_models_for_probe(&mut models);

        // mini should be first (tier 0), sonnet next (tier 1), then opus/o3 last (tier 3)
        assert_eq!(models[0].id, "gpt-4.1-mini");
        assert_eq!(models[1].id, "claude-sonnet-4");
        // opus and o3 are both tier 3; order between them is stable
        assert!(models[2].id == "claude-opus-4" || models[2].id == "o3-pro");
    }
}
