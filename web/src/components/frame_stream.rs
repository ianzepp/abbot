// Left panel showing raw kernel frame stream.

use leptos::prelude::*;

use crate::bus::Frame;
use crate::state::AppState;

fn is_tick_event(frame: &Frame) -> bool {
    if frame.op != "event" {
        return false;
    }
    frame.data.as_ref()
        .and_then(|d| d.get("kind"))
        .and_then(|k| k.as_str())
        .map(|k| k == "SIGTICK")
        .unwrap_or(false)
}

#[component]
pub fn FrameStream() -> impl IntoView {
    let state = expect_context::<AppState>();
    let state_for_clear = state.clone();

    view! {
        <div class="frame-stream">
            <div class="frame-stream-header">
                <span class="frame-stream-title">"Frames"</span>
                <button
                    class="frame-stream-clear"
                    on:click=move |_| state_for_clear.clear_frames()
                >
                    "Clear"
                </button>
            </div>
            <div class="frame-stream-list">
                <For
                    each=move || {
                        state.frames.get()
                            .into_iter()
                            .filter(|f| !is_tick_event(f))
                            .collect::<Vec<_>>()
                    }
                    key=|frame| frame.id.clone()
                    children=move |frame| {
                        view! { <FrameItem frame=frame /> }
                    }
                />
            </div>
        </div>
    }
}

fn extract_summary(frame: &Frame) -> String {
    let data = match &frame.data {
        Some(d) => d,
        None => return String::new(),
    };

    // For events, show the kind
    if frame.op == "event" {
        if let Some(kind) = data.get("kind").and_then(|v| v.as_str()) {
            return kind.to_string();
        }
    }

    // For errors, show the code or message
    if frame.op == "error" {
        if let Some(code) = data.get("code").and_then(|v| v.as_str()) {
            return code.to_string();
        }
        if let Some(msg) = data.get("message").and_then(|v| v.as_str()) {
            return truncate(msg, 50);
        }
    }

    // Common fields to look for
    let fields = ["need", "goal", "path", "text", "content", "tool", "message", "query"];
    for field in fields {
        if let Some(val) = data.get(field).and_then(|v| v.as_str()) {
            return truncate(val, 60);
        }
    }

    // For tool calls, show tool name and maybe first arg
    if let Some(tool) = data.get("tool").and_then(|v| v.as_str()) {
        return format!("tool:{}", tool);
    }

    String::new()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

fn format_data_for_tooltip(data: &Option<serde_json::Value>) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(d) = data {
        if let Some(obj) = d.as_object() {
            for (k, v) in obj {
                let val = match v {
                    serde_json::Value::String(s) => truncate(s, 80),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Null => "null".to_string(),
                    _ => serde_json::to_string(v).unwrap_or_default(),
                };
                rows.push((k.clone(), val));
            }
        }
    }
    rows
}

#[component]
fn FrameItem(frame: Frame) -> impl IntoView {
    let op_class = match frame.op.as_str() {
        "req" => "frame-op-req",
        "ok" => "frame-op-ok",
        "done" => "frame-op-done",
        "error" => "frame-op-error",
        "item" => "frame-op-item",
        "event" => "frame-op-event",
        "redirect" => "frame-op-redirect",
        _ => "frame-op-other",
    };

    let name = frame.name.clone().unwrap_or_default();
    let summary = extract_summary(&frame);
    let actor = frame.actor.clone().unwrap_or_default();
    let short_id = frame.id.chars().take(8).collect::<String>();
    let full_id = frame.id.clone();
    let parent_id = frame.parent_id.clone().unwrap_or_default();
    let data_rows = format_data_for_tooltip(&frame.data);
    let op_for_tooltip = frame.op.clone();
    let name_for_tooltip = frame.name.clone().unwrap_or_else(|| "-".to_string());

    view! {
        <div class="frame-item">
            <div class="frame-item-header">
                <span class={format!("frame-op {}", op_class)}>{frame.op.clone()}</span>
                <span class="frame-name">{name}</span>
            </div>
            {(!summary.is_empty()).then(|| view! {
                <div class="frame-item-summary">{summary}</div>
            })}
            <div class="frame-item-meta">
                <span class="frame-id">{short_id}</span>
                {(!actor.is_empty()).then(|| view! {
                    <span class="frame-actor">{actor.clone()}</span>
                })}
            </div>
            <div class="frame-tooltip">
                <table class="frame-tooltip-table">
                    <tbody>
                        <tr>
                            <td class="frame-tooltip-key">"op"</td>
                            <td class="frame-tooltip-val">{op_for_tooltip}</td>
                        </tr>
                        <tr>
                            <td class="frame-tooltip-key">"name"</td>
                            <td class="frame-tooltip-val">{name_for_tooltip}</td>
                        </tr>
                        <tr>
                            <td class="frame-tooltip-key">"id"</td>
                            <td class="frame-tooltip-val frame-tooltip-mono">{full_id}</td>
                        </tr>
                        {(!parent_id.is_empty()).then(|| view! {
                            <tr>
                                <td class="frame-tooltip-key">"parent"</td>
                                <td class="frame-tooltip-val frame-tooltip-mono">{parent_id.clone()}</td>
                            </tr>
                        })}
                        {(!actor.is_empty()).then(|| view! {
                            <tr>
                                <td class="frame-tooltip-key">"actor"</td>
                                <td class="frame-tooltip-val">{actor}</td>
                            </tr>
                        })}
                        {data_rows.into_iter().map(|(k, v)| view! {
                            <tr>
                                <td class="frame-tooltip-key">{k}</td>
                                <td class="frame-tooltip-val">{v}</td>
                            </tr>
                        }).collect_view()}
                    </tbody>
                </table>
            </div>
        </div>
    }
}
