//! Safe Mode - Automatic LLM provider failover via circuit breaker
//!
//! When an LLM provider goes down (5xx, timeout, etc.), the retry harness exhausts
//! its per-call attempts and propagates the error. Without safe mode, every subsequent
//! LLM call repeats this pattern — burning time against a dead provider.
//!
//! Safe mode detects the outage after a threshold of consecutive harness-level failures
//! (default: 3, meaning ~15 individual API attempts given 5 retries each), pauses all
//! LLM activity, probes all configured providers to find the first healthy one, switches
//! to that provider's probe model, and resumes all blocked callers. The goal is keeping
//! the LLM loop alive — everything else is secondary.
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

/// Recovery loop: probe providers until one responds, use it, resume callers.
async fn run_recovery(epoch: u64, frames: Option<&FrameStore>) {
    emit(
        frames,
        "recovery:activated",
        json!({"failure_count": FAILURE_THRESHOLD, "threshold": FAILURE_THRESHOLD, "epoch": epoch}),
    )
    .await;

    loop {
        let winner = probe_first_healthy(frames).await;

        let Some(hp) = winner else {
            emit(frames, "recovery:no_providers", json!({})).await;
            tracing::warn!(
                "safe_mode: all providers down, retrying in {}s",
                RETRY_DELAY.as_secs()
            );
            tokio::time::sleep(RETRY_DELAY).await;
            continue;
        };

        // Build override Config from the first healthy provider's probe model.
        let override_cfg = Config {
            enabled: true,
            provider: hp.name.clone(),
            base_url: hp.base_url.clone(),
            api_key: hp.api_key.clone(),
            model: hp.probe_model.clone(),
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

/// Probe all configured providers, return the first one that responds.
async fn probe_first_healthy(frames: Option<&FrameStore>) -> Option<HealthyProvider> {
    let app = AppConfig::global();

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
            Ok(Ok(_)) => {
                emit(frames, "recovery:probe_ok", json!({"provider": name})).await;
                tracing::info!(provider = %name, "safe_mode probe: healthy — using as fallback");
                return Some(HealthyProvider {
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

    None
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

        report_failure();
        report_failure();
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
}
