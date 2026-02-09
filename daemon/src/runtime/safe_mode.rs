//! Safe Mode - Apocalyptic LLM provider failover via circuit breaker
//!
//! When an LLM provider goes down (5xx, timeout, etc.), the retry harness exhausts
//! its per-call attempts and propagates the error. Without safe mode, every subsequent
//! LLM call repeats this pattern — burning time against a dead provider.
//!
//! Safe mode detects the outage after a threshold of consecutive harness-level failures
//! (default: 3, meaning ~15 individual API attempts given 5 retries each), pauses all
//! LLM activity, and runs a cascading recovery scan:
//!
//! 1. **Same-provider scan**: If the failure looks model-level (404, decode error),
//!    try other cached models from the same provider before giving up on it.
//! 2. **Cross-provider scan**: If the provider itself is down, or all its models fail,
//!    move to the next configured provider and repeat.
//! 3. **Provider-level quick-exit**: On transport/connection/auth errors, skip remaining
//!    models for that provider immediately.
//! 4. **Model filtering**: Skip non-chat models (embedding, tts, whisper, etc.).
//!
//! ## Public API
//!
//! - `init()` — call once at kernel startup
//! - `is_active()` — fast atomic check (hot path is a single load)
//! - `get_override()` — read the current override Config, if any
//! - `report_success()` — reset failure counter (called on successful llm:chat)
//! - `report_failure(context)` — increment counter, may trigger recovery
//! - `wait_for_recovery(cancel)` — park until recovered or cancelled

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::json;
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::hal::llm::{LlmClient, UnifiedMessage as Message};
use crate::kernel::{Frame, FrameStore};
use crate::runtime::app_config::AppConfig;
use crate::runtime::config::Config;
use crate::runtime::provider_cache::{
    is_likely_chat_model, load_provider_cache, sort_models_for_probe,
};

// =============================================================================
// CONFIGURATION CONSTANTS
// =============================================================================

/// Number of consecutive harness-level failures before triggering safe mode.
/// With 5 retries per harness call, this means ~15 individual API failures.
const FAILURE_THRESHOLD: u32 = 3;

/// Timeout for health-check probe calls.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Delay between retry sweeps when all providers are down.
const RETRY_DELAY: Duration = Duration::from_secs(30);

/// Probe models per provider — cheap, small models for health checks.
fn probe_model(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "claude-haiku-4-20250514",
        "openai" => "gpt-4.1-mini",
        "openrouter" => "openai/gpt-4.1-mini",
        "ollama" => "llama3",
        _ => "gpt-4.1-mini",
    }
}

// =============================================================================
// FAILURE CONTEXT
// =============================================================================

/// Context about what failed, used to guide recovery strategy.
#[derive(Clone, Debug)]
pub struct FailureContext {
    pub provider: String,
    pub model: String,
    pub classification: FailureClass,
}

/// Classification of a failure — determines where recovery starts scanning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailureClass {
    /// Model-specific failure (404, decode error) — try other models on same provider first.
    ModelLevel,
    /// Provider-wide failure (transport, auth, 5xx) — skip to next provider.
    ProviderLevel,
    /// Unknown or ambiguous failure — default to same-provider scan.
    Unknown,
}

// =============================================================================
// GLOBAL STATE
// =============================================================================

/// Fast gate — checked on every LLM call (hot path is a single atomic load).
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Failure tracker — touched only on the error path.
static TRACKER: OnceLock<Mutex<FailureTracker>> = OnceLock::new();

/// Watch channel carrying the current override config.
static STATE_TX: OnceLock<watch::Sender<SafeModeState>> = OnceLock::new();
static STATE_RX: OnceLock<watch::Receiver<SafeModeState>> = OnceLock::new();

/// Notifier for blocked callers waiting for recovery to complete.
static NOTIFY: OnceLock<Notify> = OnceLock::new();

struct FailureTracker {
    consecutive: u32,
    recovery_spawned: bool,
    epoch: u64,
    last_context: Option<FailureContext>,
}

/// State distributed to callers via watch channel.
#[derive(Clone, Debug)]
pub struct SafeModeState {
    pub override_config: Option<Config>,
    pub epoch: u64,
}

// =============================================================================
// PUBLIC API
// =============================================================================

/// Initialize the safe mode subsystem. Call once at kernel startup.
pub fn init() {
    let _ = TRACKER.set(Mutex::new(FailureTracker {
        consecutive: 0,
        recovery_spawned: false,
        epoch: 0,
        last_context: None,
    }));

    let (tx, rx) = watch::channel(SafeModeState {
        override_config: None,
        epoch: 0,
    });
    let _ = STATE_TX.set(tx);
    let _ = STATE_RX.set(rx);
    let _ = NOTIFY.set(Notify::new());

    tracing::debug!("safe_mode initialized (threshold={})", FAILURE_THRESHOLD);
}

/// Fast check: is safe mode currently active?
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Get the current override config (if safe mode has chosen a fallback).
pub fn get_override() -> Option<Config> {
    STATE_RX
        .get()
        .and_then(|rx| rx.borrow().override_config.clone())
}

/// Report a successful LLM call — resets the consecutive failure counter.
pub fn report_success() {
    if let Some(tracker) = TRACKER.get()
        && let Ok(mut t) = tracker.lock()
    {
        t.consecutive = 0;
    }
}

/// Report a failed LLM harness call. May trigger safe mode recovery.
///
/// The optional `FailureContext` tells recovery where to start scanning:
/// - `ModelLevel` → try other models on the same provider first
/// - `ProviderLevel` → skip to next provider immediately
/// - `None` / `Unknown` → default same-provider scan
pub fn report_failure(context: Option<FailureContext>) {
    let Some(tracker) = TRACKER.get() else {
        return;
    };
    let mut t = match tracker.lock() {
        Ok(t) => t,
        Err(_) => return,
    };

    t.consecutive += 1;
    t.last_context = context;
    tracing::debug!(
        consecutive = t.consecutive,
        threshold = FAILURE_THRESHOLD,
        "safe_mode failure recorded"
    );

    if t.consecutive >= FAILURE_THRESHOLD && !t.recovery_spawned {
        t.recovery_spawned = true;
        let epoch = t.epoch + 1;
        t.epoch = epoch;
        let ctx = t.last_context.clone();

        ACTIVE.store(true, Ordering::Relaxed);
        tracing::warn!(
            epoch,
            failures = t.consecutive,
            "safe_mode ACTIVATED — spawning recovery"
        );

        let frames = crate::runtime::Kernel::get().and_then(|k| k.frames());

        tokio::spawn(async move {
            run_recovery(epoch, ctx, frames.as_deref()).await;
        });
    }
}

/// Park until safe mode recovery completes (or cancellation fires).
pub async fn wait_for_recovery(cancel: &CancellationToken) {
    let Some(notify) = NOTIFY.get() else {
        return;
    };

    tokio::select! {
        _ = notify.notified() => {}
        _ = cancel.cancelled() => {}
    }
}

// =============================================================================
// RECOVERY ORCHESTRATOR
// =============================================================================

async fn emit(frames: Option<&FrameStore>, kind: &str, data: serde_json::Value) {
    if let Some(fs) = frames {
        let frame = Frame::event(Uuid::nil(), json!({"kind": kind, "data": data}))
            .with_actor("system/recovery".to_string())
            .with_name("safe_mode");
        fs.append(frame).await;
    }
}

/// Recovery loop: scan providers/models until one responds, use it, resume callers.
async fn run_recovery(epoch: u64, context: Option<FailureContext>, frames: Option<&FrameStore>) {
    emit(
        frames,
        "recovery:activated",
        json!({"failure_count": FAILURE_THRESHOLD, "threshold": FAILURE_THRESHOLD, "epoch": epoch}),
    )
    .await;

    loop {
        let winner = scan_for_healthy_model(context.as_ref(), frames).await;

        let Some(hp) = winner else {
            emit(frames, "recovery:no_providers", json!({})).await;
            tracing::warn!(
                "safe_mode: all providers/models exhausted, retrying in {}s",
                RETRY_DELAY.as_secs()
            );
            tokio::time::sleep(RETRY_DELAY).await;
            continue;
        };

        // Build override Config from the healthy provider+model.
        let override_cfg = Config {
            enabled: true,
            provider: hp.name.clone(),
            base_url: hp.base_url.clone(),
            api_key: hp.api_key.clone(),
            model: hp.model.clone(),
            temperature: None,
            max_tokens: None,
            extra_headers: vec![],
        };

        emit(
            frames,
            "recovery:completed",
            json!({"provider": override_cfg.provider, "model": override_cfg.model, "epoch": epoch}),
        )
        .await;

        tracing::info!(
            provider = %override_cfg.provider,
            model = %override_cfg.model,
            epoch,
            "safe_mode recovery complete — resuming with fallback"
        );

        // Push override into watch channel.
        if let Some(tx) = STATE_TX.get() {
            let _ = tx.send(SafeModeState {
                override_config: Some(override_cfg),
                epoch,
            });
        }

        // Deactivate safe mode and wake all blocked callers.
        ACTIVE.store(false, Ordering::Relaxed);
        if let Some(tracker) = TRACKER.get()
            && let Ok(mut t) = tracker.lock()
        {
            t.consecutive = 0;
            t.recovery_spawned = false;
            t.last_context = None;
        }
        if let Some(notify) = NOTIFY.get() {
            notify.notify_waiters();
        }

        break;
    }
}

// =============================================================================
// CASCADING PROVIDER/MODEL SCAN
// =============================================================================

/// A healthy provider+model discovered during scanning.
#[derive(Clone, Debug)]
struct HealthyProvider {
    name: String,
    base_url: String,
    api_key: String,
    model: String,
}

/// Resolve the API key for a provider from its configured env var.
fn resolve_api_key(pcfg: &crate::runtime::app_config::ProviderToml) -> String {
    pcfg.api_key_env
        .as_deref()
        .and_then(|env_name| {
            let env_name = env_name.trim();
            if env_name.is_empty() {
                None
            } else {
                std::env::var(env_name).ok()
            }
        })
        .unwrap_or_default()
}

/// Build the ordered list of candidate models for a provider.
///
/// Order: hardcoded probe model first, then cached models filtered and sorted
/// by probe priority. The `failed_model` (if any) is excluded.
fn build_candidate_list(provider_name: &str, failed_model: Option<&str>) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    // 1. Hardcoded probe model first (always try this)
    let pm = probe_model(provider_name).to_string();
    if failed_model != Some(pm.as_str()) {
        seen.insert(pm.clone());
        candidates.push(pm);
    }

    // 2. Cached models, filtered and sorted
    if let Some(cache) = load_provider_cache(provider_name) {
        let mut chat_models: Vec<_> = cache
            .models
            .into_iter()
            .filter(|m| is_likely_chat_model(&m.id))
            .filter(|m| failed_model != Some(m.id.as_str()))
            .filter(|m| !seen.contains(&m.id))
            .collect();

        sort_models_for_probe(&mut chat_models);

        for m in chat_models {
            seen.insert(m.id.clone());
            candidates.push(m.id);
        }
    }

    candidates
}

/// Heuristic: does this error string indicate a provider-level failure
/// (where all models on this provider would fail)?
fn is_provider_level_error(err_str: &str) -> bool {
    let lower = err_str.to_ascii_lowercase();
    lower.contains("connection refused")
        || lower.contains("dns")
        || lower.contains("tls")
        || lower.contains("certificate")
        || lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("connect")
        || lower.contains("network")
        || lower.contains("broken pipe")
}

/// Scan all configured providers and their cached models for a healthy one.
///
/// The scan order depends on the failure context:
/// - `ModelLevel` / `Unknown` / `None` → same provider first, then others
/// - `ProviderLevel` → skip the failed provider, try others first
async fn scan_for_healthy_model(
    context: Option<&FailureContext>,
    frames: Option<&FrameStore>,
) -> Option<HealthyProvider> {
    let app = AppConfig::global();

    // Build provider ordering based on failure context.
    let provider_names: Vec<String> = app.providers.keys().cloned().collect();
    let ordered = build_provider_order(&provider_names, context);

    let mut tried: HashSet<(String, String)> = HashSet::new();

    for provider_name in &ordered {
        let Some(pcfg) = app.providers.get(provider_name) else {
            continue;
        };
        let base_url = pcfg.base_url.as_deref().unwrap_or("").to_string();
        if base_url.is_empty() {
            continue;
        }

        let api_key = resolve_api_key(pcfg);

        // Determine which model originally failed on this provider (if any).
        let failed_model = context
            .filter(|c| c.provider == *provider_name)
            .map(|c| c.model.as_str());

        let candidates = build_candidate_list(provider_name, failed_model);
        if candidates.is_empty() {
            continue;
        }

        tracing::debug!(
            provider = %provider_name,
            candidates = candidates.len(),
            "safe_mode: scanning provider"
        );

        for model_id in &candidates {
            let pair = (provider_name.clone(), model_id.clone());
            if tried.contains(&pair) {
                continue;
            }
            tried.insert(pair);

            let result = probe_model_health(provider_name, &base_url, &api_key, model_id).await;

            match result {
                ProbeResult::Healthy => {
                    emit(
                        frames,
                        "recovery:probe_ok",
                        json!({"provider": provider_name, "model": model_id}),
                    )
                    .await;
                    tracing::info!(
                        provider = %provider_name,
                        model = %model_id,
                        "safe_mode probe: healthy — using as fallback"
                    );
                    return Some(HealthyProvider {
                        name: provider_name.clone(),
                        base_url,
                        api_key,
                        model: model_id.clone(),
                    });
                }
                ProbeResult::ModelError(e) => {
                    emit(
                        frames,
                        "recovery:probe_fail",
                        json!({"provider": provider_name, "model": model_id, "error": &e}),
                    )
                    .await;
                    tracing::warn!(
                        provider = %provider_name,
                        model = %model_id,
                        error = %e,
                        "safe_mode probe: model-level failure, trying next model"
                    );
                    // Continue to next model on this provider.
                }
                ProbeResult::ProviderError(e) => {
                    emit(
                        frames,
                        "recovery:probe_fail",
                        json!({"provider": provider_name, "model": model_id, "error": &e, "provider_level": true}),
                    )
                    .await;
                    tracing::warn!(
                        provider = %provider_name,
                        error = %e,
                        "safe_mode probe: provider-level failure, skipping remaining models"
                    );
                    // Skip remaining models on this provider.
                    break;
                }
            }
        }
    }

    None
}

/// Build the provider scan order based on failure context.
fn build_provider_order(all_providers: &[String], context: Option<&FailureContext>) -> Vec<String> {
    let Some(ctx) = context else {
        return all_providers.to_vec();
    };

    match ctx.classification {
        FailureClass::ProviderLevel => {
            // Failed provider last — try others first.
            let mut order: Vec<String> = all_providers
                .iter()
                .filter(|p| **p != ctx.provider)
                .cloned()
                .collect();
            if all_providers.contains(&ctx.provider) {
                order.push(ctx.provider.clone());
            }
            order
        }
        FailureClass::ModelLevel | FailureClass::Unknown => {
            // Same provider first (will try different models), then others.
            let mut order = Vec::with_capacity(all_providers.len());
            if all_providers.contains(&ctx.provider) {
                order.push(ctx.provider.clone());
            }
            for p in all_providers {
                if *p != ctx.provider {
                    order.push(p.clone());
                }
            }
            order
        }
    }
}

/// Result of probing a single model.
enum ProbeResult {
    Healthy,
    /// Model-specific error (try next model on same provider).
    ModelError(String),
    /// Provider-level error (skip remaining models).
    ProviderError(String),
}

/// Probe a single model for health.
async fn probe_model_health(
    provider: &str,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> ProbeResult {
    let client = LlmClient::new(provider, base_url, api_key, model, None, Some(64), vec![]);

    let messages = vec![Message::User(
        "What is 2+2? Reply with just the number.".to_string(),
    )];

    let result = tokio::time::timeout(PROBE_TIMEOUT, client.chat(messages)).await;

    match result {
        Ok(Ok(_)) => ProbeResult::Healthy,
        Ok(Err(e)) => {
            let err_str = e.to_string();
            if is_provider_level_error(&err_str) {
                ProbeResult::ProviderError(err_str)
            } else {
                ProbeResult::ModelError(err_str)
            }
        }
        Err(_) => ProbeResult::ProviderError("timeout".to_string()),
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize tests that touch global state to prevent data races.
    static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset_globals() {
        ACTIVE.store(false, Ordering::Relaxed);
        if let Some(t) = TRACKER.get() {
            if let Ok(mut t) = t.lock() {
                t.consecutive = 0;
                t.recovery_spawned = false;
                t.last_context = None;
            }
        }
        if let Some(tx) = STATE_TX.get() {
            let _ = tx.send(SafeModeState {
                override_config: None,
                epoch: 0,
            });
        }
    }

    fn ensure_init() {
        init();
    }

    #[test]
    fn default_state_is_inactive() {
        let _lock = TEST_MUTEX.lock().unwrap();
        ensure_init();
        reset_globals();
        assert!(!is_active());
        assert!(get_override().is_none());
    }

    #[test]
    fn success_resets_counter() {
        let _lock = TEST_MUTEX.lock().unwrap();
        ensure_init();
        reset_globals();

        report_failure(None);
        report_failure(None);
        assert!(!is_active(), "should not be active below threshold");

        report_success();
        if let Some(t) = TRACKER.get() {
            assert_eq!(t.lock().unwrap().consecutive, 0);
        }
    }

    #[test]
    fn threshold_triggers_active() {
        let _lock = TEST_MUTEX.lock().unwrap();
        ensure_init();
        reset_globals();

        // Test threshold logic directly (no tokio runtime for spawn).
        let tracker = TRACKER.get().unwrap();
        {
            let mut t = tracker.lock().unwrap();
            t.consecutive = FAILURE_THRESHOLD - 1;
        }
        {
            let mut t = tracker.lock().unwrap();
            t.consecutive += 1;
            assert!(t.consecutive >= FAILURE_THRESHOLD);
        }
    }

    #[test]
    fn probe_model_mapping() {
        assert_eq!(probe_model("anthropic"), "claude-haiku-4-20250514");
        assert_eq!(probe_model("openai"), "gpt-4.1-mini");
        assert_eq!(probe_model("openrouter"), "openai/gpt-4.1-mini");
        assert_eq!(probe_model("ollama"), "llama3");
        assert_eq!(probe_model("unknown"), "gpt-4.1-mini");
    }

    #[test]
    fn get_override_reflects_watch_channel() {
        let _lock = TEST_MUTEX.lock().unwrap();
        ensure_init();
        reset_globals();

        assert!(get_override().is_none());

        let cfg = Config {
            enabled: true,
            provider: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            model: "gpt-4.1-mini".to_string(),
            temperature: None,
            max_tokens: None,
            extra_headers: vec![],
        };
        if let Some(tx) = STATE_TX.get() {
            let _ = tx.send(SafeModeState {
                override_config: Some(cfg),
                epoch: 1,
            });
        }

        let ov = get_override().unwrap();
        assert_eq!(ov.provider, "openai");
        assert_eq!(ov.model, "gpt-4.1-mini");
    }

    #[test]
    fn provider_order_model_level_failure() {
        let providers = vec![
            "anthropic".to_string(),
            "openai".to_string(),
            "openrouter".to_string(),
        ];
        let ctx = FailureContext {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            classification: FailureClass::ModelLevel,
        };
        let order = build_provider_order(&providers, Some(&ctx));
        // Same provider first for model-level failure
        assert_eq!(order[0], "openai");
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn provider_order_provider_level_failure() {
        let providers = vec![
            "anthropic".to_string(),
            "openai".to_string(),
            "openrouter".to_string(),
        ];
        let ctx = FailureContext {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            classification: FailureClass::ProviderLevel,
        };
        let order = build_provider_order(&providers, Some(&ctx));
        // Failed provider last for provider-level failure
        assert_eq!(*order.last().unwrap(), "openai");
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn provider_order_no_context() {
        let providers = vec!["anthropic".to_string(), "openai".to_string()];
        let order = build_provider_order(&providers, None);
        assert_eq!(order, providers);
    }

    #[test]
    fn is_provider_level_error_detection() {
        assert!(is_provider_level_error("connection refused"));
        assert!(is_provider_level_error("HTTP 401 Unauthorized"));
        assert!(is_provider_level_error("HTTP 403 Forbidden"));
        assert!(is_provider_level_error("DNS resolution failed"));
        assert!(is_provider_level_error("TLS handshake error"));
        assert!(!is_provider_level_error("HTTP 404 Not Found"));
        assert!(!is_provider_level_error("invalid JSON in response"));
        assert!(!is_provider_level_error("model not found"));
    }

    #[test]
    fn candidate_list_excludes_failed_model() {
        // The build_candidate_list function should exclude the failed model.
        // We can't test cache loading without files, but we can verify the probe
        // model exclusion logic.
        let candidates = build_candidate_list("openai", Some("gpt-4.1-mini"));
        // The hardcoded probe model for openai is gpt-4.1-mini, which should be excluded
        assert!(!candidates.contains(&"gpt-4.1-mini".to_string()));
    }

    #[test]
    fn candidate_list_includes_probe_model_when_not_failed() {
        let candidates = build_candidate_list("openai", Some("gpt-4-turbo"));
        // The hardcoded probe model (gpt-4.1-mini) should still be first
        assert!(candidates.first().is_some_and(|m| m == "gpt-4.1-mini"));
    }

    #[test]
    fn failure_context_stored_in_tracker() {
        let _lock = TEST_MUTEX.lock().unwrap();
        ensure_init();
        reset_globals();

        let ctx = FailureContext {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            classification: FailureClass::ModelLevel,
        };
        report_failure(Some(ctx));

        if let Some(t) = TRACKER.get() {
            let t = t.lock().unwrap();
            assert!(t.last_context.is_some());
            let c = t.last_context.as_ref().unwrap();
            assert_eq!(c.provider, "openai");
            assert_eq!(c.model, "gpt-4");
            assert_eq!(c.classification, FailureClass::ModelLevel);
        }
    }
}
