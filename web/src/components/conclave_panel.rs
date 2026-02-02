// ConclavePanel component - displays conclave details.

use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;

use crate::api::get_conclave;

#[derive(Debug, Clone, Deserialize)]
struct RoomMessage {
    mind: String,
    content: String,
    round: i32,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct NeedVote {
    need: String,
    priority: String,
    votes: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WantEntry {
    want: String,
    priority: String,
    proposer: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct MemoryOp {
    kind: String,
    content: String,
    pattern: String,
    proposer: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RoomDecision {
    #[serde(default)]
    needs: Vec<NeedVote>,
    #[serde(default)]
    wants: Vec<WantEntry>,
    #[serde(default)]
    ltm_ops: Vec<MemoryOp>,
    #[serde(default)]
    self_ops: Vec<MemoryOp>,
}

fn format_transcript(transcript: &[RoomMessage]) -> String {
    if transcript.is_empty() {
        return "*No transcript recorded*".to_string();
    }

    let mut current_round = -1;
    let mut lines = Vec::new();

    for msg in transcript {
        if msg.round != current_round {
            current_round = msg.round;
            lines.push(format!("\n## Round {}\n", current_round + 1));
        }
        lines.push(format!("### {}\n", msg.mind));
        lines.push(format!("{}\n", msg.content));
    }

    lines.join("\n")
}

fn format_decision(decision: &RoomDecision) -> String {
    let mut sections = Vec::new();

    if !decision.needs.is_empty() {
        sections.push("## Needs Created\n".to_string());
        for n in &decision.needs {
            sections.push(format!("- **[{}]** {}", n.priority, n.need));
            if !n.votes.is_empty() {
                sections.push(format!("  - Votes: {}", n.votes.join(", ")));
            }
        }
        sections.push(String::new());
    }

    if !decision.wants.is_empty() {
        sections.push("## Wants Added\n".to_string());
        for w in &decision.wants {
            sections.push(format!(
                "- **[{}]** {} (by {})",
                w.priority, w.want, w.proposer
            ));
        }
        sections.push(String::new());
    }

    if !decision.ltm_ops.is_empty() {
        sections.push("## LTM Updates\n".to_string());
        for op in &decision.ltm_ops {
            let content = if op.content.is_empty() {
                &op.pattern
            } else {
                &op.content
            };
            sections.push(format!("- **{}**: {} (by {})", op.kind, content, op.proposer));
        }
        sections.push(String::new());
    }

    if !decision.self_ops.is_empty() {
        sections.push("## Self Updates\n".to_string());
        for op in &decision.self_ops {
            let content = if op.content.is_empty() {
                &op.pattern
            } else {
                &op.content
            };
            sections.push(format!("- **{}**: {} (by {})", op.kind, content, op.proposer));
        }
        sections.push(String::new());
    }

    if sections.is_empty() {
        "*No decisions made*".to_string()
    } else {
        sections.join("\n")
    }
}

#[component]
pub fn ConclavePanel(conclave_id: String) -> impl IntoView {
    let (data, set_data) = signal::<Option<Result<String, String>>>(None);

    let conclave_id_clone = conclave_id.clone();
    Effect::new(move |_| {
        let id = conclave_id_clone.clone();
        spawn_local(async move {
            match get_conclave(&id).await {
                Ok(detail) => {
                    let transcript: Vec<RoomMessage> =
                        serde_json::from_str(&detail.transcript).unwrap_or_default();
                    let decision: RoomDecision =
                        serde_json::from_str(&detail.decision).unwrap_or_default();

                    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(
                        detail.created_at as f64,
                    ));
                    let formatted_date =
                        date.to_locale_string("en-US", &wasm_bindgen::JsValue::UNDEFINED);

                    let markdown = format!(
                        r#"# Conclave: {}

**Status:** {}
**Time:** {}

---

# Transcript

{}

---

# Decisions

{}"#,
                        detail.id,
                        detail.status,
                        formatted_date.as_string().unwrap_or_default(),
                        format_transcript(&transcript),
                        format_decision(&decision)
                    );

                    set_data.set(Some(Ok(markdown)));
                }
                Err(e) => set_data.set(Some(Err(e.to_string()))),
            }
        });
    });

    view! {
        <div class="conclave-panel">
            {move || {
                match data.get() {
                    None => view! {
                        <div class="loading">"Loading..."</div>
                    }.into_any(),
                    Some(Err(e)) => view! {
                        <div class="error">{e}</div>
                    }.into_any(),
                    Some(Ok(markdown)) => view! {
                        <div class="conclave-panel-content markdown-content">
                            <pre>{markdown}</pre>
                        </div>
                    }.into_any(),
                }
            }}
        </div>
    }
}
