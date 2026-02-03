// Root App component with monitoring-first layout.
//
// Single document view area controlled by bottom tab bar.
// Overwatch view shows trace timeline, chat views show scoped conversations.

use leptos::prelude::*;

use crate::bus::use_bus;
use crate::components::{BottomTabBar, ChatView, OverwatchView, StatusBar};
use crate::state::{AppState, TabType};

#[component]
pub fn App() -> impl IntoView {
    let state = AppState::new();

    provide_context(state.clone());

    use_bus(state);

    view! {
        <div class="app-container">
            <DocumentContent />
            <BottomTabBar />
            <StatusBar />
        </div>
    }
}

#[component]
fn DocumentContent() -> impl IntoView {
    let state = expect_context::<AppState>();

    view! {
        <div class="document-view">
            {move || {
                match state.active_tab_info().map(|t| t.tab_type) {
                    Some(TabType::Overwatch) => view! { <OverwatchView /> }.into_any(),
                    Some(TabType::ScopeChat(scope)) => view! { <ChatView scope=scope /> }.into_any(),
                    Some(TabType::SessionChat(short)) => {
                        let scope = format!("session/{}", short);
                        view! { <ChatView scope=scope /> }.into_any()
                    }
                    None => view! { <div class="empty-view">"No tab selected"</div> }.into_any(),
                }
            }}
        </div>
    }
}
