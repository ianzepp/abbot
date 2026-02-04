// Frame inspector — right panel showing selected frame details.
//
// Features the "specimen card" with handwritten script font for the
// frame name, plus structured metadata and JSON payload display.

use leptos::prelude::*;

use crate::bus::Frame;
use crate::state::AppState;

#[component]
pub fn FrameInspector() -> impl IntoView {
    let state = expect_context::<AppState>();

    view! {
        <div class="inspector">
            {move || {
                match state.selected_frame.get() {
                    Some(frame) => view! { <InspectorContent frame=frame /> }.into_any(),
                    None => view! { <InspectorEmpty /> }.into_any(),
                }
            }}
        </div>
    }
}

#[component]
fn InspectorEmpty() -> impl IntoView {
    view! {
        <div class="inspector-empty">
            "SELECT_FRAME_TO_INSPECT"
        </div>
    }
}

#[component]
fn InspectorContent(frame: Frame) -> impl IntoView {
    let frame_id = frame.id.chars().take(8).collect::<String>().to_uppercase();
    let frame_name = frame.name.clone().unwrap_or_else(|| "unknown".into());
    let actor = frame.actor.clone().unwrap_or_else(|| "-".into()).to_uppercase();

    view! {
        <div class="inspector-content">
            <InspectorHeader frame_id=frame_id.clone() />
            <SpecimenCard name=frame_name.clone() actor=actor.clone() />
            <TabRow />
            <DataPanel frame=frame.clone() />
            <MetadataSection frame=frame />
        </div>
    }
}

#[component]
fn InspectorHeader(frame_id: String) -> impl IntoView {
    let now = js_sys::Date::new_0();
    let date = format!(
        "{:04}-{:02}-{:02}",
        now.get_full_year(),
        now.get_month() + 1,
        now.get_date()
    );
    let time = format!(
        "{:02}:{:02}:{:02}",
        now.get_hours(),
        now.get_minutes(),
        now.get_seconds()
    );

    view! {
        <div class="detail-header">
            <div class="detail-id">{frame_id}</div>
            <div class="detail-title">"KERNEL_FRAME_RECORD"</div>
            <div class="detail-timestamp">
                {date}<br />{time}
            </div>
        </div>
    }
}

#[component]
fn SpecimenCard(name: String, actor: String) -> impl IntoView {
    view! {
        <div class="specimen-card">
            <div class="specimen-label">"FRAME_IDENTIFIER / SYSCALL_NAME"</div>
            <div class="specimen-name">{name}</div>
            <div class="specimen-attribution">"ACTOR: "{actor}</div>
        </div>
    }
}

#[component]
fn TabRow() -> impl IntoView {
    view! {
        <div class="panel-tab-row">
            <span class="panel-tab active">"OVERVIEW"</span>
            <span class="panel-tab">"DATA"</span>
            <span class="panel-tab">"TRACE"</span>
        </div>
    }
}

#[component]
fn DataPanel(frame: Frame) -> impl IntoView {
    let json = frame.data
        .as_ref()
        .map(|d| serde_json::to_string_pretty(d).unwrap_or_else(|_| "{}".into()))
        .unwrap_or_else(|| "null".into());

    view! {
        <div class="frame-data-section">
            <div class="frame-data-title">"FRAME_PAYLOAD"</div>
            <div class="frame-data-table">
                <pre style="margin: 0; padding: 12px; font-size: 11px; white-space: pre-wrap; word-break: break-word;">
                    {json}
                </pre>
            </div>
        </div>
    }
}

#[component]
fn MetadataSection(frame: Frame) -> impl IntoView {
    let name = frame.name.clone().unwrap_or_else(|| "-".into()).to_uppercase();
    let op = frame.op.to_uppercase();
    let parent_id = frame.parent_id.clone()
        .map(|p| p.chars().take(12).collect::<String>().to_uppercase())
        .unwrap_or_else(|| "-".into());
    let actor = frame.actor.clone().unwrap_or_else(|| "-".into()).to_uppercase();

    view! {
        <div style="margin-top: 24px;">
            <div class="frame-data-title">{name.clone()}</div>
            <div class="kv-list">
                <div class="kv-row">
                    <span class="kv-label">"OP:"</span>
                    <span class="kv-value">{op}</span>
                </div>
                <div class="kv-row">
                    <span class="kv-label">"PARENT_ID:"</span>
                    <span class="kv-value">{parent_id}</span>
                </div>
                <div class="kv-row">
                    <span class="kv-label">"ACTOR:"</span>
                    <span class="kv-value">{actor}</span>
                </div>
                <div class="kv-row">
                    <span class="kv-label">"FRAME_ID:"</span>
                    <span class="kv-value">{frame.id.chars().take(12).collect::<String>().to_uppercase()}</span>
                </div>
            </div>
        </div>
    }
}
