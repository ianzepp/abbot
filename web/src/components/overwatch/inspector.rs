// Frame inspector showing details of selected frame.

use leptos::prelude::*;

use crate::bus::Frame;
use crate::state::AppState;

#[component]
pub fn Inspector(selected_frame_id: RwSignal<Option<String>>) -> impl IntoView {
    let state = expect_context::<AppState>();

    let selected_frame = move || {
        let frame_id = selected_frame_id.get()?;
        state.frames.get().into_iter().find(|f| f.id == frame_id)
    };

    view! {
        <div class="inspector">
            <div class="inspector-header">
                <span class="inspector-title">"Inspector"</span>
            </div>
            <div class="inspector-content">
                {move || match selected_frame() {
                    Some(frame) => view! { <FrameDetails frame=frame state=state.clone() /> }.into_any(),
                    None => view! {
                        <div class="inspector-empty">"Select a frame to inspect"</div>
                    }.into_any(),
                }}
            </div>
        </div>
    }
}

#[component]
fn FrameDetails(frame: Frame, state: AppState) -> impl IntoView {
    let frame_id = frame.id.clone();
    let short_id = frame_id.chars().take(8).collect::<String>();
    let parent_id = frame.parent_id.clone();
    let short_parent = parent_id.as_ref().map(|p| p.chars().take(8).collect::<String>());
    let op = frame.op.clone();
    let name = frame.name.clone().unwrap_or_else(|| "-".to_string());
    let actor = frame.actor.clone().unwrap_or_default();
    let data_rows = format_data(&frame.data);

    let scope_from_actor = actor.clone();
    let can_open_chat = !actor.is_empty() && actor != "unknown";

    view! {
        <div class="frame-details">
            <table class="frame-details-table">
                <tbody>
                    <tr>
                        <td class="detail-key">"id"</td>
                        <td class="detail-val detail-mono">{short_id}</td>
                    </tr>
                    <tr>
                        <td class="detail-key">"op"</td>
                        <td class="detail-val">{op}</td>
                    </tr>
                    <tr>
                        <td class="detail-key">"name"</td>
                        <td class="detail-val">{name}</td>
                    </tr>
                    {short_parent.map(|p| view! {
                        <tr>
                            <td class="detail-key">"parent"</td>
                            <td class="detail-val detail-mono">{p}</td>
                        </tr>
                    })}
                    {(!actor.is_empty()).then(|| view! {
                        <tr>
                            <td class="detail-key">"actor"</td>
                            <td class="detail-val">{actor.clone()}</td>
                        </tr>
                    })}
                </tbody>
            </table>

            {can_open_chat.then(|| {
                let scope = scope_from_actor.clone();
                view! {
                    <div class="inspector-actions">
                        <button
                            class="inspector-action-btn"
                            on:click=move |_| state.open_scope_chat(&scope)
                        >
                            "Open chat for this scope"
                        </button>
                    </div>
                }
            })}

            {(!data_rows.is_empty()).then(|| view! {
                <div class="frame-data-section">
                    <div class="frame-data-title">"Data"</div>
                    <table class="frame-data-table">
                        <tbody>
                            {data_rows.into_iter().map(|(k, v)| view! {
                                <tr>
                                    <td class="data-key">{k}</td>
                                    <td class="data-val">{v}</td>
                                </tr>
                            }).collect_view()}
                        </tbody>
                    </table>
                </div>
            })}
        </div>
    }
}

fn format_data(data: &Option<serde_json::Value>) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(d) = data {
        if let Some(obj) = d.as_object() {
            for (k, v) in obj {
                let val = match v {
                    serde_json::Value::String(s) => truncate(s, 100),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Null => "null".to_string(),
                    _ => {
                        let json = serde_json::to_string(v).unwrap_or_default();
                        truncate(&json, 100)
                    }
                };
                rows.push((k.clone(), val));
            }
        }
    }
    rows
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
