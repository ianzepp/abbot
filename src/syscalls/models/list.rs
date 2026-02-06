use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

pub struct ModelsList;

impl ModelsList {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct ProviderCache {
    provider: String,
    #[allow(dead_code)]
    fetched_at: String,
    models: Vec<CachedModel>,
}

#[derive(Deserialize)]
struct CachedModel {
    id: String,
    name: Option<String>,
    context_window: Option<u64>,
    #[serde(default)]
    input_cost: Option<f64>,
    #[serde(default)]
    output_cost: Option<f64>,
}

fn providers_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".config").join("abbot").join("providers"))
}

#[async_trait]
impl Syscall for ModelsList {
    fn name(&self) -> &'static str {
        "models:list"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let mut out: Vec<serde_json::Value> = Vec::new();
        if let Some(dir) = providers_dir() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("json") {
                        continue;
                    }
                    let Ok(raw) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    let Ok(cache) = serde_json::from_str::<ProviderCache>(&raw) else {
                        continue;
                    };

                    for m in cache.models {
                        let id = match cache.provider.as_str() {
                            "openrouter" => format!("openrouter/{}", m.id.trim_matches('/')),
                            p => format!("{}/{}", p, m.id.trim_matches('/')),
                        };
                        out.push(json!({
                            "id": id,
                            "name": m.name,
                            "provider": cache.provider,
                            "context_window": m.context_window,
                            "input_cost": m.input_cost,
                            "output_cost": m.output_cost,
                        }));
                    }
                }
            }
        }

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"models": out})))
            .await;

        Ok(())
    }
}
