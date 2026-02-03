// Status bar showing connection state and frame count.

use leptos::prelude::*;

use crate::state::AppState;

fn is_tick_event(frame: &crate::bus::Frame) -> bool {
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
pub fn StatusBar() -> impl IntoView {
    let state = expect_context::<AppState>();

    let connection_status = move || {
        if state.connected.get() {
            "Connected"
        } else {
            "Disconnected"
        }
    };

    let connection_class = move || {
        if state.connected.get() {
            "status-connected"
        } else {
            "status-disconnected"
        }
    };

    let frame_count = move || {
        state.frames.get().iter().filter(|f| !is_tick_event(f)).count()
    };

    let tick_count = move || {
        state.frames.get().iter().filter(|f| is_tick_event(f)).count()
    };

    view! {
        <div class="status-bar">
            <span class=connection_class>{connection_status}</span>
            <span class="status-frames">{move || format!("{} frames", frame_count())}</span>
            <span class="status-ticks">{move || format!("{} ticks", tick_count())}</span>
        </div>
    }
}
