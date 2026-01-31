use std::time::Duration;

use tokio::time::timeout;

use crate::history::Store;
use crate::llm::{
    ChatMessage, ChatToolResult, OpenAICompatClient, OpenAICompatDecodeError, OpenAICompatHttpError,
    OpenAICompatTransportError, ToolSpec,
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
    llm: &OpenAICompatClient,
    messages: Vec<ChatMessage>,
    tools: Vec<ToolSpec>,
    tool_choice: serde_json::Value,
    policy: RetryPolicy,
    mut on_retry: F,
) -> Result<ChatToolResult, HarnessError>
where
    F: FnMut(usize, &str),
{
    let mut last_err: Option<String> = None;

    for attempt in 0..policy.max_attempts {
        let call = timeout(
            policy.timeout,
            llm.chat_with_tools(messages.clone(), Some(tools.clone()), Some(tool_choice.clone())),
        )
        .await;

        match call {
            Ok(Ok(res)) => return Ok(res),
            Ok(Err(e)) => {
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
                        tokio::time::sleep(policy.backoff(attempt)).await;
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
                        tokio::time::sleep(policy.backoff(attempt)).await;
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
                        tokio::time::sleep(policy.backoff(attempt)).await;
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
                    tokio::time::sleep(policy.backoff(attempt)).await;
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
