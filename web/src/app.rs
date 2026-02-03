// Root App component with 2-panel layout.
//
// Left panel: Raw kernel frame stream
// Right panel: Tabbed views (Chat, Activity)
// Bottom: Status bar

use leptos::prelude::*;

use crate::bus::use_bus;
use crate::components::{CenterPanel, FrameStream, StatusBar};
use crate::state::AppState;

#[component]
pub fn App() -> impl IntoView {
    let state = AppState::new();

    provide_context(state.clone());

    use_bus(state);

    view! {
        <div class="app-container">
            <div class="app-layout">
                <FrameStream />
                <CenterPanel />
            </div>
            <StatusBar />
        </div>
    }
}
