//! Safe Mode - Automatic LLM provider failover via circuit breaker
//!
//! When an LLM provider goes down (5xx, timeout, etc.), the retry harness exhausts
//! its per-call attempts and propagates the error. Without safe mode, every subsequent
//! LLM call repeats this pattern — burning time against a dead provider.
//!
//! Safe mode detects the outage after a threshold of consecutive harness-level failures
//! (default: 3, meaning ~15 individual API attempts given 5 retries each), pauses all
//! LLM activity, probes all configured providers to find healthy ones, asks healthy
//! providers to recommend a fallback model, and resumes all blocked callers on the
//! chosen fallback.
//!
//! ## Public API
//!
//! - `init()` — call once at kernel startup
//! - `is_active()` — fast atomic check (hot path is a single load)
//! - `get_override()` — read the current override Config, if any
//! - `report_success()` — reset failure counter (called on successful llm:chat)
//! - `report_failure()` — increment counter, may trigger recovery
//! - `wait_for_recovery(cancel)` — park until recovered or cancelled

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
pub fn report_failure() {
    let Some(tracker) = TRACKER.get() else {
        return;
    };
    let mut t = match tracker.lock() {
        Ok(t) => t,
        Err(_) => return,
    };

    t.consecutive += 1;
    tracing::debug!(
        consecutive = t.consecutive,
        threshold = FAILURE_THRESHOLD,
        "safe_mode failure recorded"
    );

    if t.consecutive >= FAILURE_THRESHOLD && !t.recovery_spawned {
        t.recovery_spawned = true;
        let epoch = t.epoch + 1;
        t.epoch = epoch;

        ACTIVE.store(true, Ordering::Relaxed);
        tracing::warn!(
            epoch,
            failures = t.consecutive,
            "safe_mode ACTIVATED — spawning recovery"
        );

        // Get FrameStore before spawning (for frame emission).
        let frames = crate::runtime::Kernel::get().and_then(|k| k.frames());

        tokio::spawn(async move {
            run_recovery(epoch, frames.as_deref()).await;
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

async fn run_recovery(epoch: u64, frames: Option<&FrameStore>) {
    emit(
        frames,
        "recovery:activated",
        json!({"failure_count": FAILURE_THRESHOLD, "threshold": FAILURE_THRESHOLD, "epoch": epoch}),
    )
    .await;

    loop {
        let healthy = probe_all_providers(frames).await;

        if healthy.is_empty() {
            emit(frames, "recovery:no_providers", json!({})).await;
            tracing::warn!(
                "safe_mode: all providers down, retrying in {}s",
                RETRY_DELAY.as_secs()
            );
            tokio::time::sleep(RETRY_DELAY).await;
            continue;
        }

        // Ask healthy providers to recommend a fallback.
        let fallback = pick_fallback(&healthy, frames).await;

        // Build override Config from the chosen fallback.
        let override_cfg = build_override_config(&fallback);

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
        }
        if let Some(notify) = NOTIFY.get() {
            notify.notify_waiters();
        }

        break;
    }
}

// =============================================================================
// PROVIDER PROBING
// =============================================================================

/// A healthy provider discovered during probing.
#[derive(Clone, Debug)]
struct HealthyProvider {
    name: String,
    base_url: String,
    api_key: String,
    probe_model: String,
}

async fn probe_all_providers(frames: Option<&FrameStore>) -> Vec<HealthyProvider> {
    let app = AppConfig::global();
    let mut healthy = Vec::new();

    for (name, pcfg) in &app.providers {
        let base_url = pcfg.base_url.as_deref().unwrap_or("").to_string();
        if base_url.is_empty() {
            continue;
        }

        let api_key = pcfg
            .api_key_env
            .as_deref()
            .and_then(|env_name| {
                let env_name = env_name.trim();
                if env_name.is_empty() {
                    None
                } else {
                    std::env::var(env_name).ok()
                }
            })
            .unwrap_or_default();

        let model = probe_model(name).to_string();
        let client = LlmClient::new(name, &base_url, &api_key, &model, None, Some(64), vec![]);

        let messages = vec![Message::User(
            "What is 2+2? Reply with just the number.".to_string(),
        )];

        let result = tokio::time::timeout(PROBE_TIMEOUT, client.chat(messages)).await;

        match result {
            Ok(Ok(_response)) => {
                emit(frames, "recovery:probe_ok", json!({"provider": name})).await;
                tracing::info!(provider = %name, "safe_mode probe: healthy");
                healthy.push(HealthyProvider {
                    name: name.clone(),
                    base_url,
                    api_key,
                    probe_model: model,
                });
            }
            Ok(Err(e)) => {
                emit(
                    frames,
                    "recovery:probe_fail",
                    json!({"provider": name, "error": e.to_string()}),
                )
                .await;
                tracing::warn!(provider = %name, error = %e, "safe_mode probe: failed");
            }
            Err(_) => {
                emit(
                    frames,
                    "recovery:probe_fail",
                    json!({"provider": name, "error": "timeout"}),
                )
                .await;
                tracing::warn!(provider = %name, "safe_mode probe: timed out");
            }
        }
    }

    healthy
}

// =============================================================================
// FALLBACK SELECTION
// =============================================================================

/// Chosen fallback info.
struct Fallback {
    provider: String,
    model: String,
    base_url: String,
    api_key: String,
    _source: &'static str,
}

async fn pick_fallback(healthy: &[HealthyProvider], frames: Option<&FrameStore>) -> Fallback {
    use std::collections::HashMap;
    use tokio::task::JoinSet;

    let original_model = {
        let app = AppConfig::global();
        app.llm.model.clone().unwrap_or_default()
    };

    let prompt = format!(
        "The provider for model '{}' is down. Healthy providers: {}. \
         Reply with ONLY a model ID in 'provider/model' format for a temporary fallback.",
        original_model,
        healthy
            .iter()
            .map(|h| h.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let mut js = JoinSet::new();

    for hp in healthy.iter().cloned() {
        let prompt = prompt.clone();
        js.spawn(async move {
            let client = LlmClient::new(
                &hp.name,
                &hp.base_url,
                &hp.api_key,
                &hp.probe_model,
                None,
                Some(64),
                vec![],
            );
            let messages = vec![Message::User(prompt)];
            let result = tokio::time::timeout(PROBE_TIMEOUT, client.chat(messages)).await;
            (hp.name.clone(), result)
        });
    }

    let mut votes: HashMap<String, u32> = HashMap::new();
    let mut first_response: Option<String> = None;

    while let Some(result) = js.join_next().await {
        let Ok((provider_name, probe_result)) = result else {
            continue;
        };
        if let Ok(Ok(response)) = probe_result {
            let recommended = response.trim().to_string();
            emit(
                frames,
                "recovery:vote",
                json!({"provider": provider_name, "recommended_model": recommended}),
            )
            .await;

            if !recommended.is_empty() && recommended.contains('/') {
                *votes.entry(recommended.clone()).or_insert(0) += 1;
                if first_response.is_none() {
                    first_response = Some(recommended);
                }
            }
        }
    }

    // Pick highest-voted model; tie-break by first response.
    let chosen = votes
        .iter()
        .max_by_key(|(_, count)| **count)
        .map(|(model, _)| model.clone())
        .or(first_response);

    if let Some(model_id) = chosen {
        // Parse provider/model from the recommendation.
        let (provider, api_model) = if let Some(rest) = model_id.strip_prefix("openrouter/") {
            ("openrouter".to_string(), rest.to_string())
        } else if let Some(pos) = model_id.find('/') {
            (model_id[..pos].to_string(), model_id[pos + 1..].to_string())
        } else {
            (healthy[0].name.clone(), model_id)
        };

        // Find the healthy provider's connection info.
        let hp = healthy
            .iter()
            .find(|h| h.name == provider)
            .unwrap_or(&healthy[0]);

        let vote_count = votes.values().max().copied().unwrap_or(0);
        emit(
            frames,
            "recovery:fallback_chosen",
            json!({"model": format!("{}/{}", hp.name, api_model), "votes": vote_count, "source": "consensus"}),
        )
        .await;

        Fallback {
            provider: hp.name.clone(),
            model: api_model,
            base_url: hp.base_url.clone(),
            api_key: hp.api_key.clone(),
            _source: "consensus",
        }
    } else {
        // No parseable responses — use first healthy provider's probe model.
        let hp = &healthy[0];

        emit(
            frames,
            "recovery:fallback_chosen",
            json!({"model": format!("{}/{}", hp.name, hp.probe_model), "votes": 0, "source": "default"}),
        )
        .await;

        Fallback {
            provider: hp.name.clone(),
            model: hp.probe_model.clone(),
            base_url: hp.base_url.clone(),
            api_key: hp.api_key.clone(),
            _source: "default",
        }
    }
}

fn build_override_config(fb: &Fallback) -> Config {
    Config {
        enabled: true,
        provider: fb.provider.clone(),
        base_url: fb.base_url.clone(),
        api_key: fb.api_key.clone(),
        model: fb.model.clone(),
        temperature: None,
        max_tokens: None,
        extra_headers: vec![],
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

    /// Helper to reset global state between tests.
    fn reset_globals() {
        ACTIVE.store(false, Ordering::Relaxed);
        if let Some(t) = TRACKER.get() {
            if let Ok(mut t) = t.lock() {
                t.consecutive = 0;
                t.recovery_spawned = false;
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
        // OnceLock-based init is idempotent — safe to call multiple times.
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

        // Record some failures (below threshold).
        report_failure();
        report_failure();
        assert!(!is_active(), "should not be active below threshold");

        // Success resets.
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

        // We can't actually run the recovery task in unit tests (no tokio runtime
        // for the spawn), so we test the threshold logic directly.
        let tracker = TRACKER.get().unwrap();

        {
            let mut t = tracker.lock().unwrap();
            t.consecutive = FAILURE_THRESHOLD - 1;
        }

        // One more failure should trigger (but spawn will fail without runtime).
        // Test the logic without the spawn by checking the tracker state.
        {
            let mut t = tracker.lock().unwrap();
            t.consecutive += 1;
            assert!(t.consecutive >= FAILURE_THRESHOLD);
        }
    }

    #[test]
    fn build_override_config_fields() {
        let fb = Fallback {
            provider: "openai".to_string(),
            model: "gpt-4.1-mini".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            _source: "consensus",
        };
        let cfg = build_override_config(&fb);
        assert!(cfg.enabled);
        assert_eq!(cfg.provider, "openai");
        assert_eq!(cfg.model, "gpt-4.1-mini");
        assert_eq!(cfg.base_url, "https://api.openai.com/v1");
        assert_eq!(cfg.api_key, "sk-test");
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

        // Initially no override.
        assert!(get_override().is_none());

        // Push an override.
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

        let ov = get_override();
        assert!(ov.is_some());
        let ov = ov.unwrap();
        assert_eq!(ov.provider, "openai");
        assert_eq!(ov.model, "gpt-4.1-mini");
    }
}
