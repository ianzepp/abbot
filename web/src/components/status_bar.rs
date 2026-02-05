// Status bar — bottom bar with connection state and system info.
//
// Field Survey Terminal style: monospace, uppercase, warm colors.

use leptos::prelude::*;

use crate::state::AppState;

#[component]
pub fn StatusBar() -> impl IntoView {
    let state = expect_context::<AppState>();

    let connection_class = move || {
        if state.connected.get() {
            "status-connected"
        } else {
            "status-disconnected"
        }
    };

    let connection_text = move || {
        if state.connected.get() {
            "ENGINE: CONNECTED"
        } else {
            "ENGINE: DISCONNECTED"
        }
    };

    let system_state = move || {
        if state.connected.get() {
            "SYSTEM_STATE: OPERATIONAL"
        } else {
            "SYSTEM_STATE: OFFLINE"
        }
    };

    let tick_display = move || {
        state.tick_seq.get()
            .map(|seq| format!("TICK: {}", seq))
            .unwrap_or_else(|| "TICK: -".to_string())
    };

    view! {
        <footer class="status-bar">
            <div class="status-bar-left">
                <div class="status-bar-item">
                    <span class=connection_class>{connection_text}</span>
                </div>
                <div class="status-bar-item">
                    <span>{system_state}</span>
                </div>
                <div class="status-bar-item">
                    <span>{tick_display}</span>
                </div>
            </div>
            <div class="status-bar-right">
                "BUILD: V0.1.0-DEV"
            </div>
        </footer>
    }
}
