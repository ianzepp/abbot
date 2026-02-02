use std::collections::{HashMap, HashSet};

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
            return Err(format!("no pending external tool call for: {key}"));
        };

        let _ = tx.send(output);
        Ok(())
    }
}
