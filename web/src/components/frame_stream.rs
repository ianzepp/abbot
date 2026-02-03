// Left panel showing raw kernel frame stream.

use leptos::prelude::*;

use crate::bus::Frame;
use crate::state::AppState;

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
                    each=move || state.frames.get()
                    key=|frame| frame.id.clone()
                    children=move |frame| {
                        view! { <FrameItem frame=frame /> }
                    }
                />
            </div>
        </div>
    }
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
    let actor = frame.actor.clone().unwrap_or_default();
    let short_id = frame.id.chars().take(8).collect::<String>();

    view! {
        <div class="frame-item">
            <div class="frame-item-header">
                <span class={format!("frame-op {}", op_class)}>{frame.op.clone()}</span>
                <span class="frame-name">{name}</span>
            </div>
            <div class="frame-item-meta">
                <span class="frame-id">{short_id}</span>
                {(!actor.is_empty()).then(|| view! {
                    <span class="frame-actor">{actor}</span>
                })}
            </div>
        </div>
    }
}
