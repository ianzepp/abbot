use std::collections::{HashMap, HashSet};

use std::collections::VecDeque;

use tokio::sync::{Mutex, RwLock, oneshot};

use crate::history::ToolRegistryTool;

#[derive(Debug, Clone)]
pub struct ExternalTool {
    pub name: String,
    pub description: String,
    pub schema_json: String,
}

#[derive(Debug, Default)]
pub struct ExternalToolManager {
    tools_by_scope: RwLock<HashMap<String, HashMap<String, ExternalTool>>>,
    pending: Mutex<HashMap<String, oneshot::Sender<String>>>,
    recent_completed: Mutex<VecDeque<String>>,
}

impl ExternalToolManager {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(scope: &str, tool_call_id: &str) -> String {
        format!("{scope}:{tool_call_id}")
    }

    pub async fn replace_tools(&self, scope: &str, tools: &[ToolRegistryTool]) {
        let mut out = HashMap::new();
        for t in tools {
            out.insert(
                t.name.clone(),
                ExternalTool {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    schema_json: t.schema_json.clone(),
                },
            );
        }
        self.tools_by_scope
            .write()
            .await
            .insert(scope.to_string(), out);
    }

    pub async fn tool_names(&self, scope: &str) -> HashSet<String> {
        self.tools_by_scope
            .read()
            .await
            .get(scope)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub async fn register_pending(
        &self,
        scope: &str,
        tool_call_id: &str,
    ) -> Result<oneshot::Receiver<String>, String> {
        let key = Self::key(scope, tool_call_id);
        let (tx, rx) = oneshot::channel::<String>();
        let mut pending = self.pending.lock().await;
        if pending.contains_key(&key) {
            return Err(format!("duplicate pending external tool call: {key}"));
        }
        pending.insert(key, tx);
        Ok(rx)
    }

    pub async fn deliver_result(
        &self,
        scope: &str,
        tool_call_id: &str,
        output: String,
    ) -> Result<(), String> {
        let key = Self::key(scope, tool_call_id);
        let tx = {
            let mut pending = self.pending.lock().await;
            pending.remove(&key)
        };

        let Some(tx) = tx else {
            // Tool results may be retried by the client (or re-sent after reconnect). Treat
            // duplicate deliveries as idempotent if we recently completed the same call.
            let recent = self.recent_completed.lock().await;
            if recent.iter().any(|k| k == &key) {
                return Ok(());
            }
            return Err(format!(
                "no pending external tool call for: {key} (possibly duplicate or daemon restart)"
            ));
        };

        let _ = tx.send(output);

        let mut recent = self.recent_completed.lock().await;
        recent.push_back(key);
        const MAX_RECENT: usize = 256;
        while recent.len() > MAX_RECENT {
            recent.pop_front();
        }
        Ok(())
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tool(name: &str) -> ToolRegistryTool {
        ToolRegistryTool {
            name: name.to_string(),
            summary: "summary".to_string(),
            description: "description".to_string(),
            schema_json: "{}".to_string(),
        }
    }

    #[tokio::test]
    async fn test_replace_tools_and_list_names() {
        let manager = ExternalToolManager::new();
        manager
            .replace_tools("main", &[sample_tool("tool_a"), sample_tool("tool_b")])
            .await;

        let names = manager.tool_names("main").await;
        assert!(names.contains("tool_a"));
        assert!(names.contains("tool_b"));
    }

    #[tokio::test]
    async fn test_register_pending_duplicate() {
        let manager = ExternalToolManager::new();
        let _ = manager.register_pending("main", "call1").await.unwrap();
        let err = manager.register_pending("main", "call1").await.unwrap_err();
        assert!(err.contains("duplicate pending external tool call"));
    }

    #[tokio::test]
    async fn test_deliver_result_idempotent() {
        let manager = ExternalToolManager::new();
        let rx = manager.register_pending("main", "call1").await.unwrap();
        manager
            .deliver_result("main", "call1", "ok".to_string())
            .await
            .unwrap();

        let out = rx.await.unwrap();
        assert_eq!(out, "ok");

        // Duplicate delivery should be treated as idempotent.
        let second = manager
            .deliver_result("main", "call1", "ok".to_string())
            .await;
        assert!(second.is_ok());
    }
}
