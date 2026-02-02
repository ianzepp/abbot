// TabBar component - displays and manages open tabs.

use leptos::prelude::*;

use crate::state::{AppState, TabType};

#[component]
pub fn TabBar() -> impl IntoView {
    let state = expect_context::<AppState>();

    let tabs = state.tabs;
    let active_tab = state.active_tab;

    view! {
        <div class="tab-bar">
            <For
                each={move || tabs.get()}
                key={|tab| tab.id.clone()}
                children={move |tab| {
                    let tab_id = tab.id.clone();
                    let tab_id_for_click = tab_id.clone();
                    let tab_type = tab.tab_type.clone();
                    let title = tab.title.clone();
                    let state_clone = state.clone();

                    let is_fixed = matches!(tab_type, TabType::Chat | TabType::Bus | TabType::Self_ | TabType::Ltm);

                    view! {
                        <TabItem
                            tab_id=tab_id.clone()
                            tab_id_for_click=tab_id_for_click
                            tab_type=tab_type
                            title=title
                            is_fixed=is_fixed
                            active_tab=active_tab
                            state=state_clone
                        />
                    }
                }}
            />
        </div>
    }
}

#[component]
fn TabItem(
    tab_id: String,
    tab_id_for_click: String,
    tab_type: TabType,
    title: String,
    is_fixed: bool,
    active_tab: RwSignal<String>,
    state: AppState,
) -> impl IntoView {
    let tab_id_for_class = tab_id.clone();
    let tab_id_for_close = tab_id.clone();
    let state_for_close = state.clone();

    view! {
        <div
            class=move || {
                let mut class = "tab".to_string();
                if active_tab.get() == tab_id_for_class {
                    class.push_str(" active");
                }
                class
            }
            on:click=move |_| state.active_tab.set(tab_id_for_click.clone())
        >
            <span class="tab-icon">
                {tab_icon(&tab_type)}
            </span>
            <span class="tab-title">{title}</span>
            {if !is_fixed {
                let state_close = state_for_close.clone();
                let tab_close = tab_id_for_close.clone();
                view! {
                    <button
                        class="tab-close"
                        title="Close"
                        on:click=move |e| {
                            e.stop_propagation();
                            state_close.close_tab(&tab_close);
                        }
                    >
                        <svg width="10" height="10" viewBox="0 0 16 16" fill="currentColor">
                            <path d="M4.5 4.5L11.5 11.5M11.5 4.5L4.5 11.5" stroke="currentColor" stroke-width="1.5" fill="none"/>
                        </svg>
                    </button>
                }.into_any()
            } else {
                view! { <></> }.into_any()
            }}
        </div>
    }
}

fn tab_icon(tab_type: &TabType) -> impl IntoView {
    match tab_type {
        TabType::Chat => view! {
            <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                <path d="M2.5 2.5C2.5 1.67157 3.17157 1 4 1H12C12.8284 1 13.5 1.67157 13.5 2.5V10.5C13.5 11.3284 12.8284 12 12 12H6.5L3.5 15V12H4C3.17157 12 2.5 11.3284 2.5 10.5V2.5Z"/>
            </svg>
        }.into_any(),
        TabType::Bus => view! {
            <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                <path d="M1 3h14v1H1V3zm0 3h14v1H1V6zm0 3h14v1H1V9zm0 3h14v1H1v-1z"/>
            </svg>
        }.into_any(),
        TabType::Self_ => view! {
            <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                <path d="M8 1a3 3 0 100 6 3 3 0 000-6zM4 9a2 2 0 00-2 2v1a2 2 0 002 2h8a2 2 0 002-2v-1a2 2 0 00-2-2H4z"/>
            </svg>
        }.into_any(),
        TabType::Ltm => view! {
            <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                <path d="M8 1.5a6.5 6.5 0 100 13 6.5 6.5 0 000-13zM8 4a.75.75 0 01.75.75v3.5a.75.75 0 01-1.5 0v-3.5A.75.75 0 018 4zm0 8a1 1 0 100-2 1 1 0 000 2z"/>
            </svg>
        }.into_any(),
        TabType::Conclave => view! {
            <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                <path d="M5 3a2 2 0 100 4 2 2 0 000-4zM11 3a2 2 0 100 4 2 2 0 000-4zM8 9a2 2 0 100 4 2 2 0 000-4zM3 8a1 1 0 011-1h2a1 1 0 010 2H4a1 1 0 01-1-1zM10 8a1 1 0 011-1h2a1 1 0 010 2h-2a1 1 0 01-1-1zM6.5 12.5a1 1 0 011-1h1a1 1 0 010 2h-1a1 1 0 01-1-1z"/>
            </svg>
        }.into_any(),
        TabType::File => view! {
            <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                <path d="M3.5 1.75C3.5 1.33579 3.83579 1 4.25 1H9.5V4.5C9.5 4.77614 9.72386 5 10 5H13.5V14.25C13.5 14.6642 13.1642 15 12.75 15H4.25C3.83579 15 3.5 14.6642 3.5 14.25V1.75ZM10.5 1.20711L13.2929 4H10.5V1.20711Z"/>
            </svg>
        }.into_any(),
    }
}
