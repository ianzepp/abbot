use super::HeadService;
use super::types::ResumeMsg;
use crate::hal::llm::ToolCall;
use crate::runtime::Kernel;
use std::sync::Arc;

impl HeadService {
    /// Wait for external tool results and resume the need.
    pub(super) async fn wait_for_external_tools_and_resume(
        self: Arc<Self>,
        pending: Vec<ToolCall>,
    ) {
        let (scope, reply_to) = {
            let active = self.active_need.lock().await;
            let Some(n) = active.as_ref() else {
                return;
            };
            let scope = n.scope.clone().unwrap_or_else(|| "main".to_string());
            let Some(reply_to) = n.reply_to else {
                return;
            };
            (scope, reply_to)
        };

        let Some(k) = Kernel::get() else {
            return;
        };
        let key = crate::kernel::TurnKey::new(scope.as_str(), reply_to);

        let mut results = Vec::new();
        for tc in pending {
            match k.turns().take_external_tool_result(&key, &tc.id).await {
                Ok(result) => results.push(result),
                Err(crate::kernel::TurnWaitError::Cancelled) => {
                    let need = {
                        let mut active = self.active_need.lock().await;
                        if let Some(n) = active.as_mut() {
                            n.wait_kind = None;
                            n.pending_external.clear();
                            Some(n.clone())
                        } else {
                            None
                        }
                    };
                    if let Some(need) = need {
                        let _ = self.resume_tx.send(ResumeMsg::Need(need)).await;
                    }
                    return;
                }
                Err(crate::kernel::TurnWaitError::NotFound) => {
                    tracing::warn!(
                        head = %self.head_id,
                        tool_call_id = %tc.id,
                        "missing external tool result; aborting resume"
                    );
                    let need = {
                        let mut active = self.active_need.lock().await;
                        if let Some(n) = active.as_mut() {
                            n.wait_kind = None;
                            n.pending_external.clear();
                            Some(n.clone())
                        } else {
                            None
                        }
                    };
                    if let Some(need) = need {
                        let _ = self.resume_tx.send(ResumeMsg::Need(need)).await;
                    }
                    return;
                }
            }
        }

        let _ = self
            .resume_tx
            .send(ResumeMsg::ExternalTools { results })
            .await;
    }
}
