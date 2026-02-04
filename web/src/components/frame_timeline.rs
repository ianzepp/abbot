// Frame timeline — the "map" viewport showing kernel frames.
//
// Each row shows: timestamp, marker (N/T/W/-), name, status, actor.
// Clicking a row selects it for inspection in the right panel.

use leptos::prelude::*;

use crate::bus::Frame;
use crate::state::AppState;

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
    view! {
        <div class="trace-timeline-header">
            <span class="trace-timeline-title">"FRAME_SURVEY_LOG"</span>
            <button class="trace-clear-btn" on:click=on_clear>"CLEAR"</button>
        </div>
    }
}

#[component]
fn TimelineContent() -> impl IntoView {
    let state = expect_context::<AppState>();

    view! {
        <div class="trace-timeline-list">
            {move || {
                state.frames.get().iter().take(100).map(|frame| {
                    view! { <TimelineRow frame=frame.clone() /> }
                }).collect_view()
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
    let name = frame.name.clone().unwrap_or_else(|| "-".into()).to_uppercase();
    let op = frame.op.to_uppercase();
    let actor = frame.actor.clone().unwrap_or_else(|| "-".into()).to_uppercase();

    let frame_id_for_class = frame.id.clone();
    let frame_id_for_dot = frame.id.clone();
    let state_for_class = state.clone();
    let state_for_dot = state.clone();

    let on_click = move |_| {
        state.select_frame(Some(frame_for_click.clone()));
    };

    let row_class = move || {
        let is_selected = state_for_class.selected_frame.get()
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
        state_for_dot.selected_frame.get()
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
            <span class="trace-summary">{actor}</span>
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
        Some(n) if n.starts_with("tool:") => "W",
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

    view! {
        <div class="timeline-meta">
            "FILTER: ALL"<br />
            "MODE: "{mode}
        </div>
    }
}
