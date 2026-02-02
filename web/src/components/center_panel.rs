// CenterPanel component - main content area with tab-based navigation.

use leptos::prelude::*;

use super::{BusPanel, ChatPanel, ConclavePanel, FileViewer, MemoryPanel, TabBar};
use crate::state::{AppState, TabType};

#[component]
pub fn CenterPanel() -> impl IntoView {
    let state = expect_context::<AppState>();

    let tabs = state.tabs;
    let active_tab = state.active_tab;

    let current_tab = move || {
        let tabs = tabs.get();
        let active = active_tab.get();
        tabs.into_iter().find(|t| t.id == active)
    };

    view! {
        <div class="panel center-panel">
            <TabBar />
            <div class="center-panel-content">
                {move || {
                    let tab = current_tab();
                    match tab.as_ref().map(|t| (&t.tab_type, t.path.as_deref())) {
                        Some((TabType::Chat, _)) => view! { <ChatPanel /> }.into_any(),
                        Some((TabType::Bus, _)) => view! { <BusPanel /> }.into_any(),
                        Some((TabType::Self_, _)) => view! {
                            <MemoryPanel
                                title="Self"
                                description="Collective identity of the conclave - values, principles, and character."
                                fetch_type="self"
                            />
                        }.into_any(),
                        Some((TabType::Ltm, _)) => view! {
                            <MemoryPanel
                                title="Long-Term Memory"
                                description="Strategic, persistent learnings managed by the conclave."
                                fetch_type="ltm"
                            />
                        }.into_any(),
                        Some((TabType::Conclave, Some(id))) => {
                            let conclave_id = id.to_string();
                            view! { <ConclavePanel conclave_id=conclave_id /> }.into_any()
                        },
                        Some((TabType::File, Some(path))) => {
                            let file_path = path.to_string();
                            view! { <FileViewer path=file_path /> }.into_any()
                        },
                        _ => view! { <div class="empty-state">"No content"</div> }.into_any(),
                    }
                }}
            </div>
        </div>
    }
}
