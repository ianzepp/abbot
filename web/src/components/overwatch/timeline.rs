// Trace timeline showing hierarchical view of frames (flat list with indentation).

use leptos::prelude::*;

use crate::bus::Frame;
use crate::state::AppState;
use super::trace_item::TraceNodeType;

#[derive(Clone)]
struct FlatNode {
    frame: Frame,
    depth: usize,
    node_type: TraceNodeType,
    summary: String,
}

fn classify_frame(frame: &Frame) -> TraceNodeType {
    let name = frame.name.as_deref().unwrap_or("");
    if name.starts_with("need:") {
        TraceNodeType::Need
    } else if name.starts_with("task:") {
        TraceNodeType::Task
    } else if name.starts_with("tool:") || frame.op == "redirect" {
        TraceNodeType::Tool
    } else {
        TraceNodeType::Other
    }
}

fn extract_summary(frame: &Frame) -> String {
    let data = match &frame.data {
        Some(d) => d,
        None => return frame.name.clone().unwrap_or_default(),
    };

    let fields = ["need", "goal", "name", "path", "text", "content", "message", "query"];
    for field in fields {
        if let Some(val) = data.get(field).and_then(|v| v.as_str()) {
            return truncate(val, 60);
        }
    }

    frame.name.clone().unwrap_or_default()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

fn flatten_frames(frames: &[Frame]) -> Vec<FlatNode> {
    use std::collections::HashMap;

    let mut depth_map: HashMap<String, usize> = HashMap::new();
    let mut result = Vec::new();

    for frame in frames.iter().rev() {
        let depth = frame
            .parent_id
            .as_ref()
            .and_then(|pid| depth_map.get(pid))
            .map(|d| d + 1)
            .unwrap_or(0);

        depth_map.insert(frame.id.clone(), depth);

        result.push(FlatNode {
            frame: frame.clone(),
            depth,
            node_type: classify_frame(frame),
            summary: extract_summary(frame),
        });
    }

    result.reverse();
    result
}

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
pub fn TraceTimeline(selected_frame_id: RwSignal<Option<String>>) -> impl IntoView {
    let state = expect_context::<AppState>();

    let flat_nodes = move || {
        let frames = state.frames.get();
        let filtered: Vec<_> = frames.into_iter().filter(|f| !is_tick_event(f)).collect();
        flatten_frames(&filtered)
    };

    view! {
        <div class="trace-timeline">
            <div class="trace-timeline-header">
                <span class="trace-timeline-title">"Trace Timeline"</span>
                <button
                    class="trace-clear-btn"
                    on:click=move |_| state.clear_frames()
                >
                    "Clear"
                </button>
            </div>
            <div class="trace-timeline-list">
                <For
                    each=flat_nodes
                    key=|node| node.frame.id.clone()
                    children=move |node| {
                        let frame_id = node.frame.id.clone();
                        let frame_id_for_click = frame_id.clone();

                        let is_selected = {
                            let frame_id = frame_id.clone();
                            move || selected_frame_id.get().as_ref() == Some(&frame_id)
                        };

                        let indent = node.depth * 16;
                        let op = node.frame.op.clone();
                        let name = node.frame.name.clone().unwrap_or_default();
                        let summary = node.summary.clone();

                        let icon = match node.node_type {
                            TraceNodeType::Need => "N",
                            TraceNodeType::Task => "T",
                            TraceNodeType::Tool => "W",
                            TraceNodeType::Other => "-",
                        };

                        let op_class = match op.as_str() {
                            "req" => "trace-op-req",
                            "ok" => "trace-op-ok",
                            "done" => "trace-op-done",
                            "error" => "trace-op-error",
                            "redirect" => "trace-op-redirect",
                            _ => "trace-op-other",
                        };

                        let node_type_class = match node.node_type {
                            TraceNodeType::Need => "trace-node-need",
                            TraceNodeType::Task => "trace-node-task",
                            TraceNodeType::Tool => "trace-node-tool",
                            TraceNodeType::Other => "trace-node-other",
                        };

                        view! {
                            <div
                                class=move || format!(
                                    "trace-node {} {}",
                                    node_type_class,
                                    if is_selected() { "selected" } else { "" }
                                )
                                style=format!("padding-left: {}px", indent + 8)
                                on:click={
                                    let frame_id = frame_id_for_click.clone();
                                    move |_| selected_frame_id.set(Some(frame_id.clone()))
                                }
                            >
                                <span class="trace-icon">{icon}</span>
                                <span class={format!("trace-op {}", op_class)}>{op.clone()}</span>
                                <span class="trace-name">{name}</span>
                                {(!summary.is_empty()).then(|| view! {
                                    <span class="trace-summary">{summary.clone()}</span>
                                })}
                            </div>
                        }
                    }
                />
            </div>
        </div>
    }
}
