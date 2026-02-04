// Top navigation bar — Field Survey Terminal style.
//
// Left group: entity/scope tabs (FRAME_INDEX, scopes)
// Right group: view mode tabs (MONITOR, CONFIG, LOGS)

use leptos::prelude::*;

use crate::state::AppState;

#[component]
pub fn NavBar() -> impl IntoView {
    let state = expect_context::<AppState>();

    view! {
        <nav class="nav-bar">
            <div class="nav-group nav-group-left">
                <span class="nav-icon">"☰"</span>
                <span class="nav-item active">"FRAME_INDEX"</span>
                {move || {
                    state.tabs.get().iter().filter(|t| t.closable).map(|tab| {
                        let display = tab.display_name().to_uppercase().replace(" ", "_");
                        view! {
                            <span class="nav-item">{display}</span>
                        }
                    }).collect_view()
                }}
            </div>
            <div class="nav-group">
                <span class="nav-icon">"⛋"</span>
                <span class="nav-item active">"MONITOR"</span>
                <span class="nav-item">"EXPLORER"</span>
                <span class="nav-item">"CONFIG"</span>
                <span class="nav-item">"LOGS"</span>
            </div>
        </nav>
    }
}
