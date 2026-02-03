// Center panel with tabs for different views.

use leptos::prelude::*;

use crate::state::{AppState, TabType};
use crate::components::TabBar;

#[component]
pub fn CenterPanel() -> impl IntoView {
    let state = expect_context::<AppState>();

    let active_tab = move || {
        let active_id = state.active_tab.get();
        state.tabs.get().into_iter().find(|t| t.id == active_id)
    };

    view! {
        <div class="center-panel">
            <TabBar />
            <div class="center-content">
                {move || {
                    match active_tab().map(|t| t.tab_type) {
                        Some(TabType::Chat) => view! { <ChatView /> }.into_any(),
                        Some(TabType::Activity) => view! { <ActivityView /> }.into_any(),
                        None => view! { <div class="empty-view">"No tab selected"</div> }.into_any(),
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn ChatView() -> impl IntoView {
    let state = expect_context::<AppState>();

    let chat_frames = move || {
        state.frames.get()
            .into_iter()
            .filter(|f| {
                // Show frames related to chat/replies
                f.name.as_deref() == Some("reply:send")
                    || f.name.as_deref() == Some("need:enqueue")
                    || (f.op == "item" && f.name.is_none())
            })
            .collect::<Vec<_>>()
    };

    view! {
        <div class="chat-view">
            <div class="chat-messages">
                <For
                    each=chat_frames
                    key=|frame| frame.id.clone()
                    children=move |frame| {
                        let text = frame.data.as_ref()
                            .and_then(|d| d.get("text").or(d.get("content")))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());

                        text.map(|t| view! {
                            <div class="chat-message">
                                <div class="chat-message-text">{t}</div>
                            </div>
                        })
                    }
                />
            </div>
        </div>
    }
}

#[component]
fn ActivityView() -> impl IntoView {
    let state = expect_context::<AppState>();

    let activity_frames = move || {
        state.frames.get()
            .into_iter()
            .filter(|f| {
                // Show high-signal frames
                matches!(f.op.as_str(), "req" | "ok" | "error" | "done" | "redirect")
            })
            .take(100)
            .collect::<Vec<_>>()
    };

    view! {
        <div class="activity-view">
            <For
                each=activity_frames
                key=|frame| frame.id.clone()
                children=move |frame| {
                    let name = frame.name.clone().unwrap_or_else(|| "-".to_string());
                    let actor = frame.actor.clone().unwrap_or_default();

                    view! {
                        <div class="activity-item">
                            <span class={format!("activity-op activity-op-{}", frame.op)}>{frame.op.clone()}</span>
                            <span class="activity-name">{name}</span>
                            {(!actor.is_empty()).then(|| view! {
                                <span class="activity-actor">{actor}</span>
                            })}
                        </div>
                    }
                }
            />
        </div>
    }
}
