use std::time::Duration;

use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::history::Store;
use crate::hal::llm::{
    AnthropicDecodeError, AnthropicHttpError, AnthropicTransportError, LlmClient,
    OpenAICompatDecodeError, OpenAICompatHttpError, OpenAICompatTransportError,
    UnifiedChatToolResult as ChatToolResult, UnifiedMessage as Message,
    UnifiedToolSpec as ToolSpec,
};

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

fn is_retryable_http(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

pub struct HarnessError {
    pub message: String,
}

/// Calls the provider with tools and retries transient errors.
///
/// - Logs retryable failures to llm_interaction when request/response text is available.
/// - Calls `on_retry(attempt, note)` for retryable failures (caller can log per-agent events).
pub async fn chat_with_tools_retry<F>(
    store: &Store,
    agent: &str,
    run_id: &str,
    iter: usize,
    llm: &LlmClient,
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
        store, agent, run_id, iter, llm, messages, tools, tool_choice, policy, on_retry, cancel,
    )
    .await
}

async fn chat_with_tools_retry_inner<F>(
    store: &Store,
    agent: &str,
    run_id: &str,
    iter: usize,
    llm: &LlmClient,
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

    let mut last_err: Option<String> = None;

    for attempt in 0..policy.max_attempts {
        if let Some(cancel) = &cancel {
            if cancel.is_cancelled() {
                return Err(HarnessError {
                    message: "cancelled".to_string(),
                });
            }
        }

        let call = timeout(
            policy.timeout,
            llm.chat_with_tools(messages.clone(), tools_opt.clone()),
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
            Err(_) => {
                return Err(HarnessError {
                    message: "cancelled".to_string(),
                });
            }
        };

        match call {
            Ok(Ok(res)) => return Ok(res),
            Ok(Err(e)) => {
                // Try OpenAI error types
                if let Some(http) = e.downcast_ref::<OpenAICompatHttpError>() {
                    let _ = store.log_llm_interaction(
                        agent,
                        run_id,
                        iter * 10 + attempt,
                        &http.request_json,
                        &http.response_text,
                    );
                    last_err = Some(http.to_string());
                    if is_retryable_http(http.status) && attempt + 1 < policy.max_attempts {
                        let note = format!("http {}", http.status);
                        on_retry(attempt, &note);
                        sleep_or_cancel(&cancel, policy.backoff(attempt)).await?;
                        continue;
                    }
                } else if let Some(t) = e.downcast_ref::<OpenAICompatTransportError>() {
                    let _ = store.log_llm_interaction(
                        agent,
                        run_id,
                        iter * 10 + attempt,
                        &t.request_json,
                        &t.message,
                    );
                    last_err = Some(t.to_string());
                    if attempt + 1 < policy.max_attempts {
                        on_retry(attempt, "transport");
                        sleep_or_cancel(&cancel, policy.backoff(attempt)).await?;
                        continue;
                    }
                } else if let Some(d) = e.downcast_ref::<OpenAICompatDecodeError>() {
                    let _ = store.log_llm_interaction(
                        agent,
                        run_id,
                        iter * 10 + attempt,
                        &d.request_json,
                        &d.response_text,
                    );
                    last_err = Some(d.to_string());
                    if attempt + 1 < policy.max_attempts {
                        on_retry(attempt, "decode");
                        sleep_or_cancel(&cancel, policy.backoff(attempt)).await?;
                        continue;
                    }
                // Try Anthropic error types
                } else if let Some(http) = e.downcast_ref::<AnthropicHttpError>() {
                    let _ = store.log_llm_interaction(
                        agent,
                        run_id,
                        iter * 10 + attempt,
                        &http.request_json,
                        &http.response_text,
                    );
                    last_err = Some(http.to_string());
                    if is_retryable_http(http.status) && attempt + 1 < policy.max_attempts {
                        let note = format!("http {}", http.status);
                        on_retry(attempt, &note);
                        sleep_or_cancel(&cancel, policy.backoff(attempt)).await?;
                        continue;
                    }
                } else if let Some(t) = e.downcast_ref::<AnthropicTransportError>() {
                    let _ = store.log_llm_interaction(
                        agent,
                        run_id,
                        iter * 10 + attempt,
                        &t.request_json,
                        &t.message,
                    );
                    last_err = Some(t.to_string());
                    if attempt + 1 < policy.max_attempts {
                        on_retry(attempt, "transport");
                        sleep_or_cancel(&cancel, policy.backoff(attempt)).await?;
                        continue;
                    }
                } else if let Some(d) = e.downcast_ref::<AnthropicDecodeError>() {
                    let _ = store.log_llm_interaction(
                        agent,
                        run_id,
                        iter * 10 + attempt,
                        &d.request_json,
                        &d.response_text,
                    );
                    last_err = Some(d.to_string());
                    if attempt + 1 < policy.max_attempts {
                        on_retry(attempt, "decode");
                        sleep_or_cancel(&cancel, policy.backoff(attempt)).await?;
                        continue;
                    }
                } else {
                    last_err = Some(e.to_string());
                }

                break;
            }
            Err(_) => {
                last_err = Some("timeout".to_string());
                if attempt + 1 < policy.max_attempts {
                    on_retry(attempt, "timeout");
                    sleep_or_cancel(&cancel, policy.backoff(attempt)).await?;
                    continue;
                }
                break;
            }
        }
    }

    Err(HarnessError {
        message: last_err.unwrap_or_else(|| "unknown".to_string()),
    })
}

async fn sleep_or_cancel(
    cancel: &Option<CancellationToken>,
    duration: Duration,
) -> Result<(), HarnessError> {
    if let Some(cancel) = cancel {
        tokio::select! {
            _ = cancel.cancelled() => {
                return Err(HarnessError { message: "cancelled".to_string() });
            }
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
        assert!(is_retryable_http(429));
        assert!(is_retryable_http(500));
        assert!(is_retryable_http(502));
        assert!(is_retryable_http(503));
        assert!(is_retryable_http(504));
        assert!(!is_retryable_http(200));
        assert!(!is_retryable_http(400));
        assert!(!is_retryable_http(401));
        assert!(!is_retryable_http(403));
        assert!(!is_retryable_http(404));
        assert!(!is_retryable_http(422));
    }

    #[test]
    fn default_llm_policy_values() {
        let p = RetryPolicy::default_llm();
        assert_eq!(p.max_attempts, 5);
        assert_eq!(p.timeout, Duration::from_secs(120));
        assert_eq!(p.base_backoff, Duration::from_millis(250));
    }
}
