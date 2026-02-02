// ToolBar component - displays current tool activity.

use leptos::prelude::*;

use crate::state::AppState;

#[component]
pub fn ToolBar() -> impl IntoView {
    let state = expect_context::<AppState>();
    let activity = state.tool_activity;

    view! {
        <div class="tool-bar" aria-live="polite" aria-atomic="true">
            <div class="tool-bar-inner">
                <span class=move || {
                    if activity.get().is_some() {
                        "tool-bar-indicator active"
                    } else {
                        "tool-bar-indicator"
                    }
                } />
                <span class="tool-bar-text">
                    {move || activity.get().map(|a| a.text).unwrap_or_default()}
                </span>
            </div>
        </div>
    }
}
