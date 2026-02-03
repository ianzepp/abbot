// Status bar showing connection state and frame count.

use leptos::prelude::*;

use crate::state::AppState;

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

    let frame_count = move || state.frames.get().len();

    view! {
        <div class="status-bar">
            <span class=connection_class>{connection_status}</span>
            <span class="status-frames">{move || format!("{} frames", frame_count())}</span>
        </div>
    }
}
