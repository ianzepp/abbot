//! NeedService - Autonomous need processor via room spawning
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! NeedService leases autonomous needs (reply_to is null) from the need queue
//! and processes them by spawning rooms. It uses an LLM planner call to decide
//! room composition, then dispatches `room:run` with the planned agents.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Throttled concurrency: A semaphore limits how many rooms run simultaneously
//! - LLM-planned composition: An LLM call decides which agents participate
//! - Autonomous only: Needs with reply_to set are handled by HeadService (interactive)
//! - Fire-and-forget: Each need spawns as a tokio task with semaphore permit

pub mod config;

pub use config::NeedConfig;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::kernel::{Frame, FrameOp};
use crate::runtime::Kernel;
use crate::runtime::llm_util::LlmFrameAccumulator;

/// Processes autonomous needs by planning and spawning rooms.
pub struct NeedService {
    workspace_root: PathBuf,
    semaphore: Arc<Semaphore>,
    need_id: String,
}

impl NeedService {
    pub fn new(workspace_root: PathBuf, need_id: &str, max_concurrent_rooms: usize) -> Self {
        Self {
            workspace_root,
            semaphore: Arc::new(Semaphore::new(max_concurrent_rooms)),
            need_id: need_id.to_string(),
        }
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    /// Main run loop: lease autonomous needs and spawn rooms for them.
    async fn run(&self) {
        tracing::debug!(need_id = %self.need_id, "need service started");

        loop {
            // Lease an autonomous need (reply_to is null)
            let need = match self.lease_autonomous_need().await {
                Some(n) => n,
                None => continue,
            };

            tracing::info!(
                need_id = %self.need_id,
                leased_need = %need.need_id,
                need = %need.prompt,
                "leased autonomous need"
            );

            // Acquire semaphore permit before spawning
            let permit = self.semaphore.clone().acquire_owned().await;
            let Ok(permit) = permit else {
                tracing::error!("semaphore closed");
                break;
            };

            let workspace = self.workspace_root.clone();
            let service_id = self.need_id.clone();

            tokio::spawn(async move {
                let _permit = permit; // held until task completes

                // Plan room composition via LLM
                let agents = match plan_room_composition(&need.prompt, &workspace).await {
                    Some(a) => a,
                    None => {
                        tracing::warn!(
                            need_id = %need.need_id,
                            "planner returned no agents, using default composition"
                        );
                        default_agents(&need.prompt)
                    }
                };

                // Spawn room via room:run syscall
                let summary = spawn_room(&need.prompt, &agents, &workspace, &service_id).await;

                // Fulfill the need with room summary
                fulfill_need(
                    &need.need_id,
                    &summary.unwrap_or_default(),
                    &workspace,
                    &service_id,
                )
                .await;
            });
        }
    }

    /// Lease an autonomous need (reply_to is null) via need:lease syscall with filter.
    async fn lease_autonomous_need(&self) -> Option<LeasedNeed> {
        let k = Kernel::get()?;
        let dispatcher = k.dispatcher().await;

        let req = Frame::req(
            "need:lease",
            json!({
                "filter": {
                    "reply_to": null,
                }
            }),
        )
        .with_actor(format!("system/{}", self.need_id));

        let mut rx =
            dispatcher.dispatch(req, self.workspace_root.clone(), CancellationToken::new());

        let frame = rx.recv().await?;
        if frame.op != FrameOp::Ok {
            return None;
        }
        let v = frame.data?;

        let need_id = v.get("need_id")?.as_str()?.to_string();
        let prompt = v.get("need")?.as_str()?.to_string();
        let context = v
            .get("context")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();

        if need_id.trim().is_empty() || prompt.trim().is_empty() {
            return None;
        }

        Some(LeasedNeed {
            need_id,
            prompt,
            context,
        })
    }
}

/// A need leased from the queue.
struct LeasedNeed {
    need_id: String,
    prompt: String,
    #[allow(dead_code)]
    context: String,
}

/// Call LLM to plan room composition for a need.
async fn plan_room_composition(
    need_text: &str,
    workspace: &std::path::Path,
) -> Option<serde_json::Value> {
    let k = Kernel::get()?;
    let dispatcher = k.dispatcher().await;

    let system = include_str!("../../prompts/need/planner.md").replace("{need}", need_text);

    let messages = json!([
        {"role": "system", "content": system},
        {"role": "user", "content": format!("Plan the room composition for this need:\n\n{}", need_text)},
    ]);

    let payload = json!({
        "messages": messages,
    });

    let req = Frame::req("chat:llm", payload).with_actor("system/need_planner");
    let mut rx = dispatcher.dispatch(req, workspace.to_path_buf(), CancellationToken::new());

    let mut acc = LlmFrameAccumulator::new();
    while let Some(frame) = rx.recv().await {
        match acc.process_frame(&frame) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => {
                tracing::error!("planner LLM call failed");
                return None;
            }
        }
    }

    let (content, _) = acc.into_parts();

    // Extract JSON array from response (may be wrapped in markdown code fences)
    let json_str = extract_json_array(&content)?;
    let agents: serde_json::Value = serde_json::from_str(&json_str).ok()?;

    if !agents.is_array() || agents.as_array()?.is_empty() {
        return None;
    }

    Some(agents)
}

/// Extract a JSON array from text that may contain markdown code fences.
fn extract_json_array(text: &str) -> Option<String> {
    let trimmed = text.trim();

    // Try direct parse first
    if trimmed.starts_with('[') {
        return Some(trimmed.to_string());
    }

    // Try extracting from code fences
    if let Some(start) = trimmed.find('[')
        && let Some(end) = trimmed.rfind(']')
    {
        return Some(trimmed[start..=end].to_string());
    }

    None
}

/// Default agent composition when planner fails.
fn default_agents(need_text: &str) -> serde_json::Value {
    json!([
        {
            "name": "coordinator",
            "role": "head",
            "system_prompt": format!("Coordinate the work needed to fulfill this need: {}", need_text),
        },
        {
            "name": "worker",
            "role": "hand",
            "system_prompt": format!("Execute the work needed: {}", need_text),
        },
    ])
}

/// Spawn a room via room:run syscall.
async fn spawn_room(
    prompt: &str,
    agents: &serde_json::Value,
    workspace: &std::path::Path,
    actor: &str,
) -> Option<String> {
    let k = Kernel::get()?;
    let dispatcher = k.dispatcher().await;

    let req = Frame::req(
        "room:run",
        json!({
            "prompt": prompt,
            "agents": agents,
            "room_type": "general",
        }),
    )
    .with_actor(format!("system/{}", actor));

    let mut rx = dispatcher.dispatch(req, workspace.to_path_buf(), CancellationToken::new());

    while let Some(frame) = rx.recv().await {
        match frame.op {
            FrameOp::Ok => {
                let summary = frame
                    .data
                    .as_ref()
                    .and_then(|d| d.get("summary"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                return summary;
            }
            FrameOp::Error => {
                tracing::error!("room:run failed for need");
                return None;
            }
            _ => {}
        }
    }

    None
}

/// Fulfill a need via need:fulfill syscall.
async fn fulfill_need(need_id: &str, summary: &str, workspace: &std::path::Path, actor: &str) {
    let Some(k) = Kernel::get() else { return };
    let dispatcher = k.dispatcher().await;

    let req = Frame::req(
        "need:fulfill",
        json!({
            "need_id": need_id,
            "summary": summary,
        }),
    )
    .with_actor(format!("system/{}", actor));

    let mut rx = dispatcher.dispatch(req, workspace.to_path_buf(), CancellationToken::new());

    // Drain the response
    while let Some(frame) = rx.recv().await {
        if matches!(frame.op, FrameOp::Ok | FrameOp::Error | FrameOp::Done) {
            break;
        }
    }
}
