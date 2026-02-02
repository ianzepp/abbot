// Root App component with 3-panel layout.
//
// Provides the main application structure: file tree (left), center panel
// with tabs, and activity panel (right), plus a status bar at the bottom.

use leptos::prelude::*;

use crate::bus::use_bus;
use crate::components::{ActivityPanel, CenterPanel, FileTree, StatusBar};
use crate::state::AppState;

#[component]
pub fn App() -> impl IntoView {
    let state = AppState::new();

    provide_context(state.clone());

    use_bus(state);

    view! {
        <div class="app-container">
            <div class="app-layout">
                <FileTree />
                <CenterPanel />
                <ActivityPanel />
            </div>
            <StatusBar />
        </div>
    }
}
