use super::HeadService;
use super::types::{WaitKind, ResumeMsg};
use crate::hal::llm::ToolCall;
use crate::runtime::Kernel;
use std::sync::Arc;

impl HeadService {
    /// Wait for internal proc tasks to complete and resume the need.
    pub(super) async fn wait_for_tasks_and_resume(self: Arc<Self>, need_id: String) {
        use futures::future::select_all;
        use std::future::Future;
        use std::pin::Pin;

        loop {
            let (pending_ids, turn_check) = {
                let active = self.active_need.lock().await;
                let Some(n) = active.as_ref() else {
                    return;
                };
                if n.need_id != need_id {
                    return;
                }
                if n.wait_kind != Some(WaitKind::Tasks) {
                    return;
                }
                (n.pending_task_ids.clone(), Some(n.clone()))
            };

            if let Some(n) = turn_check {
                if self.is_turn_cancelled(&n).await {
                    let need = {
                        let mut active = self.active_need.lock().await;
                        if let Some(n) = active.as_mut() {
                            n.wait_kind = None;
                            n.pending_task_ids.clear();
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

            if pending_ids.is_empty() {
                let _ = self
                    .resume_tx
                    .send(ResumeMsg::TasksDone {
                        need_id: need_id.clone(),
                    })
                    .await;
                return;
            }

            let mut unfinished: Vec<String> = Vec::new();
            let Some(k) = Kernel::get() else {
                return;
            };

            if let Some(ems) = k.ems() {
                for id in &pending_ids {
                    let status = {
                        let ems = ems.lock().unwrap();
                        let rows = ems
                            .select(
                                "tasks",
                                Some(&serde_json::json!({"id": id.as_str()})),
                                None,
                                None,
                                Some(1),
                                None,
                            )
                            .ok()
                            .and_then(|mut r| r.pop());
                        rows.and_then(|r| r.get("status").and_then(|v| v.as_str().map(String::from)))
                    };
                    match status.as_deref() {
                        Some("completed") | Some("failed") => {}
                        _ => unfinished.push(id.clone()),
                    }
                }
            }

            if unfinished.is_empty() {
                let _ = self
                    .resume_tx
                    .send(ResumeMsg::TasksDone {
                        need_id: need_id.clone(),
                    })
                    .await;
                return;
            }

            let notifies: Vec<Arc<tokio::sync::Notify>> = {
                let mut out = Vec::new();
                for id in &unfinished {
                    out.push(k.tasks().watcher(id).await);
                }
                out
            };

            let waits: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = notifies
                .into_iter()
                .map(|n| {
                    Box::pin(async move { n.notified().await })
                        as Pin<Box<dyn Future<Output = ()> + Send>>
                })
                .collect();

            let _ = select_all(waits).await;
        }
    }

    /// Wait for external tool results and resume the need.
    pub(super) async fn wait_for_external_tools_and_resume(self: Arc<Self>, pending: Vec<ToolCall>) {
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

        let _ = self.resume_tx.send(ResumeMsg::ExternalTools { results }).await;
    }
}
