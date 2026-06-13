//! Preflight check system for daemon startup validation.
//!
//! Runs config, endpoint, and database checks before the kernel initializes.
//! Results are written to `<workspace>/preflight.log` and emitted via tracing.

use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

use super::app_config::{AppConfig, WorkspacePaths};
use super::config::Config;
use super::{HandConfig, HeadConfig, RoomConfig};

// ─── Types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckSeverity {
    Critical,
    Warning,
}

impl fmt::Display for CheckSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Critical => write!(f, "critical"),
            Self::Warning => write!(f, "warning"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Fail(String),
    Skip(String),
}

impl fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pass => write!(f, "pass"),
            Self::Fail(msg) => write!(f, "FAIL      {msg}"),
            Self::Skip(msg) => write!(f, "skip      {msg}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: String,
    pub severity: CheckSeverity,
    pub status: CheckStatus,
    pub duration: Duration,
}

#[derive(Debug, Clone)]
pub struct AgentSummary {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub has_api_key: bool,
}

#[derive(Debug, Clone)]
pub struct StatLine {
    pub label: String,
    pub count: i64,
    pub detail: String,
}

pub struct PreflightReport {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub agents: Vec<AgentSummary>,
    pub results: Vec<CheckResult>,
    pub stats: Vec<StatLine>,
}

impl PreflightReport {
    fn has_critical_failures(&self) -> bool {
        self.results.iter().any(|r| {
            r.severity == CheckSeverity::Critical && matches!(r.status, CheckStatus::Fail(_))
        })
    }

    fn to_log_string(&self) -> String {
        let overall = if self.has_critical_failures() {
            "FAIL"
        } else {
            "PASS"
        };

        let mut out = String::new();
        out.push_str(&format!(
            "# Preflight: {}\n# {}\n\n",
            overall,
            self.timestamp.to_rfc3339(),
        ));

        // Agent config summary
        for a in &self.agents {
            let key_status = if a.has_api_key { "key set" } else { "no key" };
            if a.provider.is_empty() && a.model.is_empty() {
                out.push_str(&format!("  {:<6} (not configured)\n", a.name));
            } else {
                out.push_str(&format!(
                    "  {:<6} {:<12} {:<34} {} ({})\n",
                    a.name, a.provider, a.model, a.base_url, key_status,
                ));
            }
        }
        out.push('\n');

        for r in &self.results {
            let ms = r.duration.as_millis();
            let badge = match (&r.status, r.severity) {
                (CheckStatus::Pass, _) => "[ OK ]",
                (CheckStatus::Skip(_), _) => "[SKIP]",
                (CheckStatus::Fail(_), CheckSeverity::Critical) => "[FAIL]",
                (CheckStatus::Fail(_), CheckSeverity::Warning) => "[WARN]",
            };
            let detail = match &r.status {
                CheckStatus::Pass => String::new(),
                CheckStatus::Fail(msg) | CheckStatus::Skip(msg) => msg.clone(),
            };
            if detail.is_empty() {
                out.push_str(&format!("{} {:<28} {:>5}ms\n", badge, r.name, ms,));
            } else {
                out.push_str(&format!(
                    "{} {:<28} {:>5}ms  {}\n",
                    badge, r.name, ms, detail,
                ));
            }
        }

        // System stats
        if !self.stats.is_empty() {
            out.push_str("\n  System\n");
            for s in &self.stats {
                if s.detail.is_empty() {
                    out.push_str(&format!("  {:<22} {:>8}\n", s.label, s.count));
                } else {
                    out.push_str(&format!(
                        "  {:<22} {:>8}  ({})\n",
                        s.label, s.count, s.detail,
                    ));
                }
            }
        }

        out
    }
}

// ─── Entry Point ────────────────────────────────────────────────────────────

/// Run all preflight checks, write results to `<workspace>/preflight.log`,
/// and return `Err` if any critical check failed.
pub async fn run_preflight(paths: &WorkspacePaths) -> Result<(), String> {
    let mut results = Vec::new();

    let agents = check_config(&mut results);
    check_endpoints(&mut results).await;
    check_api(&mut results).await;
    check_databases(paths, &mut results).await;
    let stats = collect_stats(paths).await;

    let report = PreflightReport {
        timestamp: chrono::Utc::now(),
        agents,
        results,
        stats,
    };

    // Emit each result via tracing
    for r in &report.results {
        match &r.status {
            CheckStatus::Pass => {
                tracing::info!(check = %r.name, severity = %r.severity, "preflight pass");
            }
            CheckStatus::Skip(reason) => {
                tracing::info!(check = %r.name, severity = %r.severity, reason = %reason, "preflight skip");
            }
            CheckStatus::Fail(reason) => {
                if r.severity == CheckSeverity::Critical {
                    tracing::error!(check = %r.name, severity = %r.severity, reason = %reason, "preflight FAIL");
                } else {
                    tracing::warn!(check = %r.name, severity = %r.severity, reason = %reason, "preflight FAIL");
                }
            }
        }
    }

    // Write preflight.log
    let log_path = paths.workspace.join("preflight.log");
    let log_content = report.to_log_string();
    if let Err(e) = std::fs::write(&log_path, &log_content) {
        tracing::warn!(error = %e, path = %log_path.display(), "failed to write preflight.log");
    }

    if report.has_critical_failures() {
        let failures: Vec<_> = report
            .results
            .iter()
            .filter(|r| {
                r.severity == CheckSeverity::Critical && matches!(r.status, CheckStatus::Fail(_))
            })
            .map(|r| {
                if let CheckStatus::Fail(msg) = &r.status {
                    format!("  {}: {}", r.name, msg)
                } else {
                    unreachable!()
                }
            })
            .collect();

        Err(format!(
            "critical preflight checks failed (see {}):\n{}",
            log_path.display(),
            failures.join("\n"),
        ))
    } else {
        tracing::info!(log = %log_path.display(), "preflight passed");
        Ok(())
    }
}

// ─── Phase 1: Config Validation ─────────────────────────────────────────────

fn check_config(results: &mut Vec<CheckResult>) -> Vec<AgentSummary> {
    let app = AppConfig::global();
    let start = Instant::now();

    // config.providers — at least one provider configured
    let providers_status = if app.providers.is_empty() {
        CheckStatus::Fail("no providers configured in [providers.*]".into())
    } else {
        CheckStatus::Pass
    };
    results.push(CheckResult {
        name: "config.providers".into(),
        severity: CheckSeverity::Critical,
        status: providers_status,
        duration: start.elapsed(),
    });

    // config.head
    let start = Instant::now();
    let head_cfg = HeadConfig::from_config();
    results.push(CheckResult {
        name: "config.head".into(),
        severity: CheckSeverity::Critical,
        status: llm_config_status(&head_cfg.llm),
        duration: start.elapsed(),
    });

    // config.hand
    let start = Instant::now();
    let hand_cfg = HandConfig::from_config();
    results.push(CheckResult {
        name: "config.hand".into(),
        severity: CheckSeverity::Critical,
        status: llm_config_status(&hand_cfg.llm),
        duration: start.elapsed(),
    });

    // config.mind
    let start = Instant::now();
    let mind_cfg = RoomConfig::from_config();
    results.push(CheckResult {
        name: "config.mind".into(),
        severity: CheckSeverity::Warning,
        status: llm_config_status(&mind_cfg.llm),
        duration: start.elapsed(),
    });

    fn summarize(name: &str, cfg: &Config) -> AgentSummary {
        AgentSummary {
            name: name.into(),
            provider: cfg.provider.clone(),
            model: cfg.model.clone(),
            base_url: cfg.base_url.clone(),
            has_api_key: !cfg.api_key.trim().is_empty(),
        }
    }

    vec![
        summarize("head", &head_cfg.llm),
        summarize("hand", &hand_cfg.llm),
        summarize("mind", &mind_cfg.llm),
    ]
}

fn llm_config_status(cfg: &Config) -> CheckStatus {
    if cfg.enabled {
        return CheckStatus::Pass;
    }

    let mut reasons = Vec::new();
    if cfg.model.trim().is_empty() {
        reasons.push("empty model");
    }
    if cfg.base_url.trim().is_empty() {
        reasons.push("empty base_url");
    }
    if !cfg.base_url.trim().is_empty()
        && !cfg.model.trim().is_empty()
        && cfg.api_key.trim().is_empty()
    {
        reasons.push("missing api_key");
    }

    if reasons.is_empty() {
        CheckStatus::Fail("disabled (unknown reason)".into())
    } else {
        CheckStatus::Fail(reasons.join(", "))
    }
}

// ─── Phase 2: Endpoint Reachability ─────────────────────────────────────────

async fn check_endpoints(results: &mut Vec<CheckResult>) {
    let head_cfg = HeadConfig::from_config();
    let hand_cfg = HandConfig::from_config();
    let mind_cfg = RoomConfig::from_config();

    let mut checked_urls: Vec<String> = Vec::new();

    // endpoint.head
    let start = Instant::now();
    let head_status = if !head_cfg.llm.enabled {
        CheckStatus::Skip("head LLM not enabled".into())
    } else {
        checked_urls.push(head_cfg.llm.base_url.clone());
        probe_endpoint(&head_cfg.llm.base_url).await
    };
    results.push(CheckResult {
        name: "endpoint.head".into(),
        severity: CheckSeverity::Warning,
        status: head_status,
        duration: start.elapsed(),
    });

    // endpoint.hand
    let start = Instant::now();
    let hand_status = if !hand_cfg.llm.enabled {
        CheckStatus::Skip("hand LLM not enabled".into())
    } else if checked_urls.contains(&hand_cfg.llm.base_url) {
        CheckStatus::Skip("same url as head".into())
    } else {
        checked_urls.push(hand_cfg.llm.base_url.clone());
        probe_endpoint(&hand_cfg.llm.base_url).await
    };
    results.push(CheckResult {
        name: "endpoint.hand".into(),
        severity: CheckSeverity::Warning,
        status: hand_status,
        duration: start.elapsed(),
    });

    // endpoint.mind
    let start = Instant::now();
    let mind_status = if !mind_cfg.llm.enabled {
        CheckStatus::Skip("mind LLM not enabled".into())
    } else if checked_urls.contains(&mind_cfg.llm.base_url) {
        CheckStatus::Skip("same url as head".into())
    } else {
        probe_endpoint(&mind_cfg.llm.base_url).await
    };
    results.push(CheckResult {
        name: "endpoint.mind".into(),
        severity: CheckSeverity::Warning,
        status: mind_status,
        duration: start.elapsed(),
    });
}

async fn probe_endpoint(base_url: &str) -> CheckStatus {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(e) => return CheckStatus::Fail(format!("http client error: {e}")),
    };

    match client.get(base_url).send().await {
        Ok(_) => CheckStatus::Pass, // Any HTTP response (even 401/404) means reachable
        Err(e) if e.is_timeout() => CheckStatus::Fail("timeout (5s)".into()),
        Err(e) if e.is_connect() => CheckStatus::Fail(format!("connection refused: {e}")),
        Err(e) => CheckStatus::Fail(format!("request failed: {e}")),
    }
}

// ─── Phase 3: API Validation ─────────────────────────────────────────────────

async fn check_api(results: &mut Vec<CheckResult>) {
    let head_cfg = HeadConfig::from_config();
    let hand_cfg = HandConfig::from_config();
    let mind_cfg = RoomConfig::from_config();

    // Deduplicate by (base_url, model, api_key) tuple.
    let mut checked: Vec<(String, String, String)> = Vec::new();

    let agents: Vec<(&str, CheckSeverity, &Config)> = vec![
        ("api.head", CheckSeverity::Critical, &head_cfg.llm),
        ("api.hand", CheckSeverity::Critical, &hand_cfg.llm),
        ("api.mind", CheckSeverity::Warning, &mind_cfg.llm),
    ];

    for (name, severity, cfg) in agents {
        let start = Instant::now();
        let status = if !cfg.enabled {
            CheckStatus::Skip(format!(
                "{} LLM not enabled",
                name.strip_prefix("api.").unwrap_or(name)
            ))
        } else {
            let key = (cfg.base_url.clone(), cfg.model.clone(), cfg.api_key.clone());
            if checked.contains(&key) {
                CheckStatus::Skip("same (base_url, model, api_key) already checked".into())
            } else {
                checked.push(key);
                probe_api(cfg).await
            }
        };
        results.push(CheckResult {
            name: name.into(),
            severity,
            status,
            duration: start.elapsed(),
        });
    }
}

async fn probe_api(cfg: &Config) -> CheckStatus {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(e) => return CheckStatus::Fail(format!("http client error: {e}")),
    };

    let (url, request) = if cfg.provider == "anthropic" {
        let base = cfg.base_url.trim_end_matches('/');
        let url = if base.ends_with("/v1") {
            format!("{}/messages", base)
        } else {
            format!("{}/v1/messages", base)
        };
        let body = serde_json::json!({
            "model": cfg.model,
            "messages": [{"role": "user", "content": "what is 2+2"}],
            "max_tokens": 32
        });
        let req = client
            .post(&url)
            .header("x-api-key", &cfg.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .body(body.to_string());
        (url, req)
    } else {
        // Prefer /chat/completions, but some completion-only models reject it.
        // For preflight we can safely fall back to /completions.
        let base = cfg.base_url.trim_end_matches('/');
        let url = format!("{}/chat/completions", base);
        let body = serde_json::json!({
            "model": cfg.model,
            "messages": [{"role": "user", "content": "what is 2+2"}],
            "max_tokens": 32
        });
        let req = client
            .post(&url)
            .header("authorization", format!("Bearer {}", cfg.api_key))
            .header("content-type", "application/json")
            .body(body.to_string());
        (url, req)
    };

    match request.send().await {
        Ok(resp) if resp.status().is_success() => CheckStatus::Pass,
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            // Fall back from /chat/completions -> /completions if the provider indicates
            // the model is completion-only.
            if cfg.provider != "anthropic"
                && (body.contains("not a chat model")
                    && (body.contains("v1/completions") || body.contains("/completions")))
            {
                let base = cfg.base_url.trim_end_matches('/');
                let url2 = format!("{}/completions", base);
                let body2 = serde_json::json!({
                    "model": cfg.model,
                    "prompt": "what is 2+2\n",
                    "max_tokens": 32
                });
                let req2 = client
                    .post(&url2)
                    .header("authorization", format!("Bearer {}", cfg.api_key))
                    .header("content-type", "application/json")
                    .body(body2.to_string());
                return match req2.send().await {
                    Ok(r2) if r2.status().is_success() => CheckStatus::Pass,
                    Ok(r2) => {
                        let s2 = r2.status();
                        let b2 = r2.text().await.unwrap_or_default();
                        let truncated = truncate_utf8(&b2, 200);
                        CheckStatus::Fail(format!("{url2} returned {s2}: {truncated}"))
                    }
                    Err(e) if e.is_timeout() => CheckStatus::Fail(format!("{url2} timeout (30s)")),
                    Err(e) if e.is_connect() => {
                        CheckStatus::Fail(format!("{url2} connection refused: {e}"))
                    }
                    Err(e) => CheckStatus::Fail(format!("{url2} request failed: {e}")),
                };
            }

            // Fall back from max_tokens -> max_completion_tokens for newer OpenAI models.
            if cfg.provider != "anthropic"
                && body.contains("max_tokens")
                && body.contains("max_completion_tokens")
                && body.contains("Unsupported parameter")
            {
                let base = cfg.base_url.trim_end_matches('/');
                let url2 = format!("{}/chat/completions", base);
                let body2 = serde_json::json!({
                    "model": cfg.model,
                    "messages": [{"role": "user", "content": "what is 2+2"}],
                    "max_completion_tokens": 32
                });
                let req2 = client
                    .post(&url2)
                    .header("authorization", format!("Bearer {}", cfg.api_key))
                    .header("content-type", "application/json")
                    .body(body2.to_string());
                return match req2.send().await {
                    Ok(r2) if r2.status().is_success() => CheckStatus::Pass,
                    Ok(r2) => {
                        let s2 = r2.status();
                        let b2 = r2.text().await.unwrap_or_default();
                        let truncated = truncate_utf8(&b2, 200);
                        CheckStatus::Fail(format!("{url2} returned {s2}: {truncated}"))
                    }
                    Err(e) if e.is_timeout() => CheckStatus::Fail(format!("{url2} timeout (30s)")),
                    Err(e) if e.is_connect() => {
                        CheckStatus::Fail(format!("{url2} connection refused: {e}"))
                    }
                    Err(e) => CheckStatus::Fail(format!("{url2} request failed: {e}")),
                };
            }

            let truncated = truncate_utf8(&body, 200);
            CheckStatus::Fail(format!("{url} returned {status}: {truncated}"))
        }
        Err(e) if e.is_timeout() => CheckStatus::Fail(format!("{url} timeout (30s)")),
        Err(e) if e.is_connect() => CheckStatus::Fail(format!("{url} connection refused: {e}")),
        Err(e) => CheckStatus::Fail(format!("{url} request failed: {e}")),
    }
}

fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = std::cmp::min(max_bytes, s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

// ─── Phase 4: Database Accessibility ────────────────────────────────────────

async fn check_databases(paths: &WorkspacePaths, results: &mut Vec<CheckResult>) {
    // database.store (Critical)
    let start = Instant::now();
    results.push(CheckResult {
        name: "database.store".into(),
        severity: CheckSeverity::Critical,
        status: probe_database(&paths.store_db).await,
        duration: start.elapsed(),
    });

    // database.frames (Warning)
    let start = Instant::now();
    results.push(CheckResult {
        name: "database.frames".into(),
        severity: CheckSeverity::Warning,
        status: probe_database(&paths.frames_db).await,
        duration: start.elapsed(),
    });

    // database.ems (Warning)
    let start = Instant::now();
    results.push(CheckResult {
        name: "database.ems".into(),
        severity: CheckSeverity::Warning,
        status: probe_database(&paths.ems_db).await,
        duration: start.elapsed(),
    });
}

async fn probe_database(path: &Path) -> CheckStatus {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);

    let pool = match SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options)
        .await
    {
        Ok(p) => p,
        Err(e) => return CheckStatus::Fail(format!("open failed: {e}")),
    };

    let result = match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&pool)
        .await
    {
        Ok(_) => CheckStatus::Pass,
        Err(e) => CheckStatus::Fail(format!("query failed: {e}")),
    };

    pool.close().await;
    result
}

// ─── System Stats ────────────────────────────────────────────────────────────

async fn collect_stats(paths: &WorkspacePaths) -> Vec<StatLine> {
    let mut stats = Vec::new();

    // Frames
    if let Some(rows) = db_count(&paths.frames_db, "frames", None).await {
        stats.push(StatLine {
            label: "frames".into(),
            count: rows,
            detail: String::new(),
        });
    }

    // EMS — entities table with kind filters (needs, tasks, wants)
    for (label, kind) in &[("needs", "need"), ("tasks", "task"), ("wants", "want")] {
        let kind_filter = format!("kind = '{kind}'");
        if let Some(total) = db_count(&paths.ems_db, "entities", Some(&kind_filter)).await {
            let pending_filter = format!("kind = '{kind}' AND status = 'pending'");
            let running_filter = format!("kind = '{kind}' AND status = 'running'");
            let pending = db_count(&paths.ems_db, "entities", Some(&pending_filter))
                .await
                .unwrap_or(0);
            let running = db_count(&paths.ems_db, "entities", Some(&running_filter))
                .await
                .unwrap_or(0);
            let mut parts = Vec::new();
            if pending > 0 {
                parts.push(format!("{pending} pending"));
            }
            if running > 0 {
                parts.push(format!("{running} running"));
            }
            stats.push(StatLine {
                label: label.to_string(),
                count: total,
                detail: parts.join(", "),
            });
        }
    }

    // Store — interesting tables
    for table in &["hand_exec", "llm_interaction", "room_schedules"] {
        if let Some(total) = db_count(&paths.store_db, table, None).await {
            stats.push(StatLine {
                label: table.to_string(),
                count: total,
                detail: String::new(),
            });
        }
    }

    stats
}

async fn db_count(db_path: &Path, table: &str, where_clause: Option<&str>) -> Option<i64> {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    if !db_path.exists() {
        return None;
    }

    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .read_only(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(2))
        .connect_with(options)
        .await
        .ok()?;

    // Validate table name (alphanumeric + underscore only)
    if !table.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }

    let sql = match where_clause {
        Some(w) => format!("SELECT COUNT(*) FROM \"{table}\" WHERE {w}"),
        None => format!("SELECT COUNT(*) FROM \"{table}\""),
    };

    let result = sqlx::query_scalar::<_, i64>(&sql)
        .fetch_one(&pool)
        .await
        .ok();

    pool.close().await;
    result
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_severity_display() {
        assert_eq!(format!("{}", CheckSeverity::Critical), "critical");
        assert_eq!(format!("{}", CheckSeverity::Warning), "warning");
    }

    #[test]
    fn check_status_display() {
        assert_eq!(format!("{}", CheckStatus::Pass), "pass");
        assert!(format!("{}", CheckStatus::Fail("oops".into())).contains("oops"));
        assert!(format!("{}", CheckStatus::Skip("dedup".into())).contains("dedup"));
    }

    fn test_agents() -> Vec<AgentSummary> {
        vec![
            AgentSummary {
                name: "head".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-20250514".into(),
                base_url: "https://api.anthropic.com".into(),
                has_api_key: true,
            },
            AgentSummary {
                name: "hand".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-20250514".into(),
                base_url: "https://api.anthropic.com".into(),
                has_api_key: true,
            },
            AgentSummary {
                name: "mind".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-20250514".into(),
                base_url: "https://api.anthropic.com".into(),
                has_api_key: true,
            },
        ]
    }

    #[test]
    fn report_has_critical_failures() {
        let report = PreflightReport {
            timestamp: chrono::Utc::now(),
            agents: test_agents(),
            results: vec![
                CheckResult {
                    name: "a".into(),
                    severity: CheckSeverity::Critical,
                    status: CheckStatus::Pass,
                    duration: Duration::ZERO,
                },
                CheckResult {
                    name: "b".into(),
                    severity: CheckSeverity::Warning,
                    status: CheckStatus::Fail("not critical".into()),
                    duration: Duration::ZERO,
                },
            ],
            stats: vec![],
        };
        assert!(!report.has_critical_failures());
    }

    #[test]
    fn report_critical_failure_detected() {
        let report = PreflightReport {
            timestamp: chrono::Utc::now(),
            agents: test_agents(),
            results: vec![CheckResult {
                name: "a".into(),
                severity: CheckSeverity::Critical,
                status: CheckStatus::Fail("boom".into()),
                duration: Duration::ZERO,
            }],
            stats: vec![],
        };
        assert!(report.has_critical_failures());
    }

    #[test]
    fn report_log_string_format() {
        let report = PreflightReport {
            timestamp: chrono::Utc::now(),
            agents: test_agents(),
            stats: vec![
                StatLine {
                    label: "frames".into(),
                    count: 1234,
                    detail: String::new(),
                },
                StatLine {
                    label: "tasks".into(),
                    count: 12,
                    detail: "3 pending, 1 running".into(),
                },
            ],
            results: vec![
                CheckResult {
                    name: "config.head".into(),
                    severity: CheckSeverity::Critical,
                    status: CheckStatus::Pass,
                    duration: Duration::from_millis(3),
                },
                CheckResult {
                    name: "endpoint.hand".into(),
                    severity: CheckSeverity::Warning,
                    status: CheckStatus::Skip("same url as head".into()),
                    duration: Duration::ZERO,
                },
                CheckResult {
                    name: "api.mind".into(),
                    severity: CheckSeverity::Warning,
                    status: CheckStatus::Fail("timeout".into()),
                    duration: Duration::from_millis(100),
                },
                CheckResult {
                    name: "database.store".into(),
                    severity: CheckSeverity::Critical,
                    status: CheckStatus::Fail("open failed".into()),
                    duration: Duration::from_millis(5),
                },
            ],
        };
        let log = report.to_log_string();
        assert!(log.contains("# Preflight: FAIL"));
        // Agent summary section
        assert!(log.contains("head"));
        assert!(log.contains("anthropic"));
        assert!(log.contains("claude-sonnet-4-20250514"));
        assert!(log.contains("key set"));
        // Check rows
        assert!(log.contains("[ OK ] config.head"));
        assert!(log.contains("[SKIP] endpoint.hand"));
        assert!(log.contains("same url as head"));
        assert!(log.contains("[WARN] api.mind"));
        assert!(log.contains("[FAIL] database.store"));
        // Stats section
        assert!(log.contains("System"));
        assert!(log.contains("frames"));
        assert!(log.contains("1234"));
        assert!(log.contains("tasks"));
        assert!(log.contains("3 pending, 1 running"));
    }

    #[test]
    fn report_unconfigured_agent() {
        let report = PreflightReport {
            timestamp: chrono::Utc::now(),
            agents: vec![AgentSummary {
                name: "mind".into(),
                provider: String::new(),
                model: String::new(),
                base_url: String::new(),
                has_api_key: false,
            }],
            results: vec![],
            stats: vec![],
        };
        let log = report.to_log_string();
        assert!(log.contains("mind"));
        assert!(log.contains("(not configured)"));
    }

    #[test]
    fn llm_config_status_reports_reasons() {
        let cfg = Config {
            enabled: false,
            provider: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            temperature: None,
            max_tokens: None,
            extra_headers: vec![],
        };
        let status = llm_config_status(&cfg);
        match status {
            CheckStatus::Fail(msg) => {
                assert!(msg.contains("empty model"));
                assert!(msg.contains("empty base_url"));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_database_with_temp() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let status = probe_database(&db_path).await;
        assert_eq!(status, CheckStatus::Pass);
    }

    #[tokio::test]
    async fn probe_database_bad_path() {
        let path = std::path::Path::new("/nonexistent/dir/test.db");
        let status = probe_database(path).await;
        assert!(matches!(status, CheckStatus::Fail(_)));
    }

    #[tokio::test]
    async fn probe_api_unreachable() {
        let cfg = Config {
            enabled: true,
            provider: "openai".to_string(),
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: "sk-test".to_string(),
            model: "gpt-4".to_string(),
            temperature: None,
            max_tokens: None,
            extra_headers: vec![],
        };
        let status = probe_api(&cfg).await;
        assert!(matches!(status, CheckStatus::Fail(_)));
    }

    #[tokio::test]
    async fn probe_api_anthropic_unreachable() {
        let cfg = Config {
            enabled: true,
            provider: "anthropic".to_string(),
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: "sk-ant-test".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            temperature: None,
            max_tokens: None,
            extra_headers: vec![],
        };
        let status = probe_api(&cfg).await;
        assert!(matches!(status, CheckStatus::Fail(_)));
    }
}
