use std::fmt;
use std::time::Duration;

use serde::Serialize;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::hal::llm::{
    AnthropicDecodeError, AnthropicHttpError, AnthropicTransportError, LlmClient,
    OpenAICompatDecodeError, OpenAICompatHttpError, OpenAICompatTransportError,
    UnifiedChatToolResult as ChatToolResult, UnifiedMessage as Message,
    UnifiedToolSpec as ToolSpec,
};
use crate::history::Store;
use crate::kernel::KernelError;

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: usize,
    pub timeout: Duration,
    pub base_backoff: Duration,
}

impl RetryPolicy {
    pub fn default_llm() -> Self {
        Self {
            max_attempts: 5,
            timeout: Duration::from_secs(120),
            base_backoff: Duration::from_millis(250),
        }
    }

    pub fn backoff(&self, attempt: usize) -> Duration {
        // 250ms, 500ms, 1000ms, 2000ms, 4000ms
        let shift = attempt.min(4) as u32;
        let mult = 1u32.checked_shl(shift).unwrap_or(u32::MAX);
        self.base_backoff.saturating_mul(mult)
    }
}

// ---------------------------------------------------------------------------
// HarnessError — structured error from the retry harness
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HarnessError {
    pub kind: HarnessKind,
    pub ctx: HarnessCtx,
    pub retry: RetryMeta,
}

#[derive(Debug, Clone)]
pub enum HarnessKind {
    Http { status: u16, body: String },
    Transport { message: String },
    Decode { message: String },
    Timeout,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct HarnessCtx {
    pub provider: String,
    pub model: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RetryMeta {
    pub attempt: usize,
    pub max_attempts: usize,
    pub retryable: bool,
}

impl HarnessError {
    /// Construct a non-retryable transport error with unknown context.
    /// Use for pre-call failures (e.g. kernel not initialized, frame errors).
    pub fn transport(message: impl Into<String>) -> Self {
        Self {
            kind: HarnessKind::Transport {
                message: message.into(),
            },
            ctx: HarnessCtx {
                provider: String::new(),
                model: String::new(),
                base_url: String::new(),
            },
            retry: RetryMeta::default(),
        }
    }
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            HarnessKind::Http { status, body } => {
                write!(f, "{} returned HTTP {status}", self.ctx.provider)?;
                if !body.is_empty() {
                    write!(f, ": {body}")?;
                }
                Ok(())
            }
            HarnessKind::Transport { message } => {
                write!(f, "{}: {message}", self.ctx.provider)
            }
            HarnessKind::Decode { message } => {
                write!(f, "{}: decode error: {message}", self.ctx.provider)
            }
            HarnessKind::Timeout => {
                write!(f, "{} request timed out", self.ctx.provider)
            }
            HarnessKind::Cancelled => {
                write!(f, "{} request cancelled", self.ctx.provider)
            }
        }
    }
}

impl std::error::Error for HarnessError {}

impl From<HarnessError> for KernelError {
    fn from(e: HarnessError) -> Self {
        let code = match &e.kind {
            HarnessKind::Http { .. }
            | HarnessKind::Transport { .. }
            | HarnessKind::Decode { .. } => "E_IO",
            HarnessKind::Timeout => "E_TIMEOUT",
            HarnessKind::Cancelled => "E_CANCELLED",
        };

        KernelError::new(code, e.to_string())
            .with_retryable(e.retry.retryable)
            .with_detail(serde_json::json!({
                "provider": e.ctx.provider,
                "model": e.ctx.model,
                "base_url": e.ctx.base_url,
                "kind": format!("{:?}", e.kind),
                "attempt": e.retry.attempt,
                "max_attempts": e.retry.max_attempts,
            }))
    }
}

// ---------------------------------------------------------------------------
// HAL error classification
// ---------------------------------------------------------------------------

fn classify_hal_error(e: &(dyn std::error::Error + Send + Sync + 'static)) -> HarnessKind {
    // OpenAI-compatible
    if let Some(h) = e.downcast_ref::<OpenAICompatHttpError>() {
        return HarnessKind::Http {
            status: h.status,
            body: truncate(&h.response_text, 512),
        };
    }
    if let Some(t) = e.downcast_ref::<OpenAICompatTransportError>() {
        return HarnessKind::Transport {
            message: t.message.clone(),
        };
    }
    if let Some(d) = e.downcast_ref::<OpenAICompatDecodeError>() {
        return HarnessKind::Decode {
            message: d.message.clone(),
        };
    }
    // Anthropic
    if let Some(h) = e.downcast_ref::<AnthropicHttpError>() {
        return HarnessKind::Http {
            status: h.status,
            body: truncate(&h.response_text, 512),
        };
    }
    if let Some(t) = e.downcast_ref::<AnthropicTransportError>() {
        return HarnessKind::Transport {
            message: t.message.clone(),
        };
    }
    if let Some(d) = e.downcast_ref::<AnthropicDecodeError>() {
        return HarnessKind::Decode {
            message: d.message.clone(),
        };
    }
    // Unknown provider error
    HarnessKind::Transport {
        message: e.to_string(),
    }
}

fn is_retryable(kind: &HarnessKind) -> bool {
    match kind {
        HarnessKind::Http { status, .. } => matches!(status, 429 | 500 | 502 | 503 | 504),
        HarnessKind::Transport { .. } => true,
        HarnessKind::Decode { .. } => true,
        HarnessKind::Timeout => true,
        HarnessKind::Cancelled => false,
    }
}

/// Extract request/response JSON from a HAL error for interaction logging.
fn hal_error_request_response<'a>(
    e: &'a (dyn std::error::Error + Send + Sync + 'static),
) -> Option<(&'a str, &'a str)> {
    if let Some(h) = e.downcast_ref::<OpenAICompatHttpError>() {
        return Some((&h.request_json, &h.response_text));
    }
    if let Some(t) = e.downcast_ref::<OpenAICompatTransportError>() {
        return Some((&t.request_json, &t.message));
    }
    if let Some(d) = e.downcast_ref::<OpenAICompatDecodeError>() {
        return Some((&d.request_json, &d.response_text));
    }
    if let Some(h) = e.downcast_ref::<AnthropicHttpError>() {
        return Some((&h.request_json, &h.response_text));
    }
    if let Some(t) = e.downcast_ref::<AnthropicTransportError>() {
        return Some((&t.request_json, &t.message));
    }
    if let Some(d) = e.downcast_ref::<AnthropicDecodeError>() {
        return Some((&d.request_json, &d.response_text));
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

/// Calls the provider with tools and retries transient errors.
///
/// - Logs retryable failures to llm_interaction when request/response text is available.
/// - Calls `on_retry(attempt, note)` for retryable failures (caller can log per-agent events).
/// - Returns a structured `HarnessError` with provider context on failure.
#[allow(clippy::too_many_arguments)]
pub async fn chat_with_tools_retry<F>(
    store: &Store,
    agent: &str,
    run_id: &str,
    iter: usize,
    llm: &LlmClient,
    ctx: HarnessCtx,
    system: Option<String>,
    messages: Vec<Message>,
    tools: Vec<ToolSpec>,
    tool_choice: serde_json::Value,
    policy: RetryPolicy,
    on_retry: F,
    cancel: Option<CancellationToken>,
) -> Result<ChatToolResult, HarnessError>
where
    F: FnMut(usize, &str),
{
    chat_with_tools_retry_inner(
        store,
        agent,
        run_id,
        iter,
        llm,
        ctx,
        system,
        messages,
        tools,
        tool_choice,
        policy,
        on_retry,
        cancel,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn chat_with_tools_retry_inner<F>(
    store: &Store,
    agent: &str,
    run_id: &str,
    iter: usize,
    llm: &LlmClient,
    ctx: HarnessCtx,
    system: Option<String>,
    messages: Vec<Message>,
    tools: Vec<ToolSpec>,
    _tool_choice: serde_json::Value,
    policy: RetryPolicy,
    mut on_retry: F,
    cancel: Option<CancellationToken>,
) -> Result<ChatToolResult, HarnessError>
where
    F: FnMut(usize, &str),
{
    let tools_opt = if tools.is_empty() {
        None
    } else {
        Some(tools.clone())
    };

    let cancelled = |ctx: &HarnessCtx, attempt: usize| HarnessError {
        kind: HarnessKind::Cancelled,
        ctx: ctx.clone(),
        retry: RetryMeta {
            attempt,
            max_attempts: policy.max_attempts,
            retryable: false,
        },
    };

    let mut last_kind: Option<HarnessKind> = None;
    let mut last_attempt: usize = 0;

    for attempt in 0..policy.max_attempts {
        if let Some(cancel) = &cancel
            && cancel.is_cancelled()
        {
            return Err(cancelled(&ctx, attempt));
        }

        let call = timeout(
            policy.timeout,
            llm.chat_with_tools(system.clone(), messages.clone(), tools_opt.clone()),
        );

        let call = match &cancel {
            Some(cancel) => tokio::select! {
                _ = cancel.cancelled() => Err(()),
                res = call => Ok(res),
            },
            None => Ok(call.await),
        };

        let call = match call {
            Ok(res) => res,
            Err(_) => return Err(cancelled(&ctx, attempt)),
        };

        match call {
            Ok(Ok(res)) => {
                let _ = store
                    .log_llm_interaction(
                        agent,
                        run_id,
                        iter * 10 + attempt,
                        res.usage.input_tokens,
                        res.usage.output_tokens,
                        &res.request_json,
                        &res.response_json,
                    )
                    .await;
                return Ok(res);
            }
            Ok(Err(e)) => {
                // Log request/response if available
                if let Some((req, resp)) = hal_error_request_response(e.as_ref()) {
                    let _ = store
                        .log_llm_interaction(agent, run_id, iter * 10 + attempt, 0, 0, req, resp)
                        .await;
                }

                let kind = classify_hal_error(e.as_ref());
                let retryable = is_retryable(&kind) && attempt + 1 < policy.max_attempts;

                if retryable {
                    let note = match &kind {
                        HarnessKind::Http { status, .. } => format!("http {status}"),
                        HarnessKind::Transport { .. } => "transport".to_string(),
                        HarnessKind::Decode { .. } => "decode".to_string(),
                        _ => "error".to_string(),
                    };
                    on_retry(attempt, &note);
                    last_kind = Some(kind);
                    last_attempt = attempt;
                    if sleep_or_cancel(&cancel, &ctx, attempt, &policy)
                        .await
                        .is_err()
                    {
                        return Err(cancelled(&ctx, attempt));
                    }
                    continue;
                }

                // Non-retryable or exhausted attempts
                last_kind = Some(kind);
                last_attempt = attempt;
                break;
            }
            Err(_) => {
                // Timeout from tokio::time::timeout
                let kind = HarnessKind::Timeout;
                let retryable = attempt + 1 < policy.max_attempts;

                if retryable {
                    on_retry(attempt, "timeout");
                    last_kind = Some(kind);
                    last_attempt = attempt;
                    if sleep_or_cancel(&cancel, &ctx, attempt, &policy)
                        .await
                        .is_err()
                    {
                        return Err(cancelled(&ctx, attempt));
                    }
                    continue;
                }

                last_kind = Some(kind);
                last_attempt = attempt;
                break;
            }
        }
    }

    let kind = last_kind.unwrap_or(HarnessKind::Transport {
        message: "unknown error".to_string(),
    });
    let retryable = is_retryable(&kind);

    Err(HarnessError {
        kind,
        ctx,
        retry: RetryMeta {
            attempt: last_attempt,
            max_attempts: policy.max_attempts,
            retryable,
        },
    })
}

async fn sleep_or_cancel(
    cancel: &Option<CancellationToken>,
    _ctx: &HarnessCtx,
    _attempt: usize,
    policy: &RetryPolicy,
) -> Result<(), ()> {
    let duration = policy.backoff(_attempt);
    if let Some(cancel) = cancel {
        tokio::select! {
            _ = cancel.cancelled() => { return Err(()); }
            _ = tokio::time::sleep(duration) => {}
        }
    } else {
        tokio::time::sleep(duration).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_exponential() {
        let p = RetryPolicy::default_llm();
        assert_eq!(p.backoff(0), Duration::from_millis(250));
        assert_eq!(p.backoff(1), Duration::from_millis(500));
        assert_eq!(p.backoff(2), Duration::from_millis(1000));
        assert_eq!(p.backoff(3), Duration::from_millis(2000));
        assert_eq!(p.backoff(4), Duration::from_millis(4000));
    }

    #[test]
    fn backoff_caps_at_shift_4() {
        let p = RetryPolicy::default_llm();
        // Attempts beyond 4 should stay at 4000ms (shift capped at 4)
        assert_eq!(p.backoff(5), Duration::from_millis(4000));
        assert_eq!(p.backoff(100), Duration::from_millis(4000));
    }

    #[test]
    fn retryable_http_status_codes() {
        let http = |status| HarnessKind::Http {
            status,
            body: String::new(),
        };
        assert!(is_retryable(&http(429)));
        assert!(is_retryable(&http(500)));
        assert!(is_retryable(&http(502)));
        assert!(is_retryable(&http(503)));
        assert!(is_retryable(&http(504)));
        assert!(!is_retryable(&http(200)));
        assert!(!is_retryable(&http(400)));
        assert!(!is_retryable(&http(401)));
        assert!(!is_retryable(&http(403)));
        assert!(!is_retryable(&http(404)));
        assert!(!is_retryable(&http(422)));
    }

    #[test]
    fn retryable_by_kind() {
        assert!(is_retryable(&HarnessKind::Transport {
            message: "conn reset".into()
        }));
        assert!(is_retryable(&HarnessKind::Decode {
            message: "bad json".into()
        }));
        assert!(is_retryable(&HarnessKind::Timeout));
        assert!(!is_retryable(&HarnessKind::Cancelled));
    }

    #[test]
    fn default_llm_policy_values() {
        let p = RetryPolicy::default_llm();
        assert_eq!(p.max_attempts, 5);
        assert_eq!(p.timeout, Duration::from_secs(120));
        assert_eq!(p.base_backoff, Duration::from_millis(250));
    }

    #[test]
    fn harness_error_display() {
        let ctx = HarnessCtx {
            provider: "anthropic".into(),
            model: "claude-sonnet".into(),
            base_url: "https://api.anthropic.com".into(),
        };
        let e = HarnessError {
            kind: HarnessKind::Http {
                status: 529,
                body: "overloaded".into(),
            },
            ctx: ctx.clone(),
            retry: RetryMeta {
                attempt: 4,
                max_attempts: 5,
                retryable: false,
            },
        };
        assert_eq!(e.to_string(), "anthropic returned HTTP 529: overloaded");

        let e2 = HarnessError {
            kind: HarnessKind::Timeout,
            ctx,
            retry: RetryMeta::default(),
        };
        assert_eq!(e2.to_string(), "anthropic request timed out");
    }

    #[test]
    fn harness_error_to_kernel_error() {
        let e = HarnessError {
            kind: HarnessKind::Http {
                status: 502,
                body: "bad gateway".into(),
            },
            ctx: HarnessCtx {
                provider: "openai".into(),
                model: "gpt-4".into(),
                base_url: "https://api.openai.com/v1".into(),
            },
            retry: RetryMeta {
                attempt: 3,
                max_attempts: 5,
                retryable: true,
            },
        };
        let ke = KernelError::from(e);
        assert_eq!(ke.code, "E_IO");
        assert!(ke.message.contains("openai"));
        assert!(ke.message.contains("502"));
        assert_eq!(ke.retryable, Some(true));

        let detail = ke.detail.unwrap();
        assert_eq!(detail["provider"], "openai");
        assert_eq!(detail["model"], "gpt-4");
        assert_eq!(detail["attempt"], 3);
        assert_eq!(detail["max_attempts"], 5);
    }

    #[test]
    fn transport_helper() {
        let e = HarnessError::transport("kernel not initialized");
        assert!(e.to_string().contains("kernel not initialized"));
        assert!(matches!(e.kind, HarnessKind::Transport { .. }));
        assert!(!e.retry.retryable);
    }
}
