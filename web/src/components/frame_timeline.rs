// Frame timeline — the "map" viewport showing kernel frames.
//
// Each row shows: timestamp, marker (N/T/W/-), name, status, actor.
// Clicking a row selects it for inspection in the right panel.

use leptos::prelude::*;

use crate::bus::Frame;
use crate::state::AppState;

fn frame_scope(frame: &Frame) -> Option<&str> {
    frame
        .trace
        .as_ref()
        .and_then(|t| t.get("scope"))
        .and_then(|s| s.as_str())
        .or_else(|| {
            frame
                .data
                .as_ref()
                .and_then(|d| d.get("scope"))
                .and_then(|s| s.as_str())
        })
}

#[component]
pub fn FrameTimeline() -> impl IntoView {
    let state = expect_context::<AppState>();

    let state_clear = state.clone();
    let state_pause = state.clone();

    let on_clear = move |_| {
        state_clear.clear_frames();
    };

    let on_toggle_pause = move |_| {
        state_pause.toggle_pause();
    };

    view! {
        <div class="frame-timeline">
            <TimelineHeader on_clear=on_clear />
            <TimelineContent />
            <TimelineControls on_toggle_pause=on_toggle_pause />
            <TimelinePagination />
            <TimelineMeta />
        </div>
    }
}

#[component]
fn TimelineHeader(on_clear: impl Fn(web_sys::MouseEvent) + 'static) -> impl IntoView {
    let state = expect_context::<AppState>();
    let state_ok = state.clone();
    let state_req = state.clone();
    let state_event = state.clone();

    let on_toggle_ok = move |_| {
        state_ok.show_ok_frames.update(|v| *v = !*v);
    };
    let on_toggle_req = move |_| {
        state_req.show_req_frames.update(|v| *v = !*v);
    };
    let on_toggle_event = move |_| {
        state_event.show_event_frames.update(|v| *v = !*v);
    };

    let ok_class = move || {
        if state.show_ok_frames.get() {
            "trace-filter-btn active"
        } else {
            "trace-filter-btn"
        }
    };
    let req_class = move || {
        if state.show_req_frames.get() {
            "trace-filter-btn active"
        } else {
            "trace-filter-btn"
        }
    };
    let event_class = move || {
        if state.show_event_frames.get() {
            "trace-filter-btn active"
        } else {
            "trace-filter-btn"
        }
    };

    view! {
        <div class="trace-timeline-header">
            <span class="trace-timeline-title">"FRAME_SURVEY_LOG"</span>
            <div class="trace-header-actions">
                <button class=ok_class on:click=on_toggle_ok>"OK"</button>
                <button class=req_class on:click=on_toggle_req>"REQ"</button>
                <button class=event_class on:click=on_toggle_event>"EVENT"</button>
                <span class="trace-header-sep"></span>
                <button class="trace-clear-btn" on:click=on_clear>"CLEAR"</button>
            </div>
        </div>
    }
}

#[component]
fn TimelineContent() -> impl IntoView {
    let state = expect_context::<AppState>();

    view! {
        <div class="trace-timeline-list">
            {move || {
                let filter = state.frame_filter.get();
                let show_ok = state.show_ok_frames.get();
                let show_req = state.show_req_frames.get();
                let show_event = state.show_event_frames.get();
                let frames: Vec<_> = state.frames.get().iter()
                    .filter(|frame| {
                        // Filter by name prefix if set
                let passes_name_filter = match &filter {
                    None => true,
                    Some(prefix) => {
                        let Some(name) = frame.name.as_deref() else {
                            return false;
                        };
                        if prefix.as_str() == "tool:" {
                            name.starts_with("tool:") || name == "chat:tool"
                        } else {
                            name.starts_with(prefix.as_str())
                        }
                    }
                };
                        // Filter by op type
                        let op = frame.op.to_lowercase();
                        let passes_op_filter = match op.as_str() {
                            "ok" => show_ok,
                            "req" => show_req,
                            "event" => show_event,
                            _ => true,
                        };
                        passes_name_filter && passes_op_filter
                    })
                    .take(100)
                    .cloned()
                    .collect();

                if frames.is_empty() {
                    view! {
                        <div class="timeline-empty">"NO_DATA"</div>
                    }.into_any()
                } else {
                    frames.into_iter()
                        .map(|frame| view! { <TimelineRow frame=frame /> })
                        .collect_view()
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
fn TimelineRow(frame: Frame) -> impl IntoView {
    let state = expect_context::<AppState>();
    let frame_for_click = frame.clone();
    let frame_for_selected = frame.clone();

    let marker = frame_marker(&frame);
    let name = frame
        .name
        .clone()
        .unwrap_or_else(|| "-".into())
        .to_uppercase();
    let op = frame.op.to_uppercase();
    let actor = frame
        .actor
        .clone()
        .unwrap_or_else(|| "-".into())
        .to_uppercase();
    let scope = frame_scope(&frame)
        .map(|s| {
            if let Some(hash) = s.strip_prefix("session/") {
                format!("@{}", &hash[..4.min(hash.len())]).to_uppercase()
            } else {
                format!("#{}", s).to_uppercase()
            }
        })
        .unwrap_or_else(|| "".to_string());

    let frame_id_for_class = frame.id.clone();
    let frame_id_for_dot = frame.id.clone();
    let state_for_class = state.clone();
    let state_for_dot = state.clone();

    let on_click = move |_| {
        state.select_frame(Some(frame_for_click.clone()));
    };

    let row_class = move || {
        let is_selected = state_for_class
            .selected_frame
            .get()
            .as_ref()
            .map(|f| f.id == frame_id_for_class)
            .unwrap_or(false);
        if is_selected {
            "trace-node selected".to_string()
        } else {
            "trace-node".to_string()
        }
    };

    let is_selected = move || {
        state_for_dot
            .selected_frame
            .get()
            .as_ref()
            .map(|f| f.id == frame_id_for_dot)
            .unwrap_or(false)
    };

    let op_class = format!("trace-op trace-op-{}", frame.op.to_lowercase());

    view! {
        <div class=row_class on:click=on_click>
            <span class="timeline-time">{frame_timestamp(&frame)}</span>
            <span class="trace-icon">{marker}</span>
            <span class="trace-name">{name}</span>
            <span class=op_class>{op}</span>
            <span class="trace-summary">{move || if scope.is_empty() {
                actor.clone()
            } else {
                format!("{} {}", scope, actor)
            }}</span>
            <span>
                {move || if is_selected() {
                    Some(view! { <span class="selected-dot"></span> })
                } else {
                    None
                }}
            </span>
        </div>
    }
}

fn frame_marker(frame: &Frame) -> &'static str {
    match frame.name.as_deref() {
        Some(n) if n.starts_with("need:") => "N",
        Some(n) if n.starts_with("task:") => "T",
        Some(n) if n.starts_with("tool:") || n == "chat:tool" => "W",
        _ => "-",
    }
}

fn frame_timestamp(_frame: &Frame) -> String {
    let now = js_sys::Date::new_0();
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        now.get_hours(),
        now.get_minutes(),
        now.get_seconds(),
        now.get_milliseconds()
    )
}

#[component]
fn TimelineControls(on_toggle_pause: impl Fn(web_sys::MouseEvent) + 'static) -> impl IntoView {
    let state = expect_context::<AppState>();
    let is_paused = move || state.paused.get();

    view! {
        <div class="timeline-controls">
            <button class="timeline-btn" on:click=on_toggle_pause>
                {move || if is_paused() { "▶" } else { "⏸" }}
            </button>
        </div>
    }
}

#[component]
fn TimelinePagination() -> impl IntoView {
    let state = expect_context::<AppState>();
    let count = move || state.frames.get().len();

    view! {
        <div class="timeline-pagination">
            "FRAMES: "{count}" OF 500"
        </div>
    }
}

#[component]
fn TimelineMeta() -> impl IntoView {
    let state = expect_context::<AppState>();
    let mode = move || if state.paused.get() { "PAUSED" } else { "LIVE" };
    let filter = move || {
        state
            .frame_filter
            .get()
            .map(|f| f.trim_end_matches(':').to_uppercase())
            .unwrap_or_else(|| "ALL".to_string())
    };

    view! {
        <div class="timeline-meta">
            "FILTER: "{filter}<br />
            "MODE: "{mode}
        </div>
    }
}
