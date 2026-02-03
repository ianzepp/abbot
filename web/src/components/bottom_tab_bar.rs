// Bottom tab bar for switching between views (Excel-style tabs).

use leptos::prelude::*;

use crate::state::AppState;

#[component]
pub fn BottomTabBar() -> impl IntoView {
    let state = expect_context::<AppState>();

    view! {
        <div class="bottom-tab-bar">
            <div class="tab-items">
                <For
                    each=move || state.tabs.get()
                    key=|tab| tab.id.clone()
                    children=move |tab| {
                        let state = state.clone();
                        let tab_id = tab.id.clone();
                        let tab_id_for_click = tab.id.clone();
                        let tab_id_for_close = tab.id.clone();
                        let display_name = tab.display_name();
                        let closable = tab.closable;

                        let is_active = {
                            let tab_id = tab_id.clone();
                            move || state.active_tab.get() == tab_id
                        };

                        let state_for_click = state.clone();
                        let state_for_close = state.clone();

                        view! {
                            <div
                                class=move || if is_active() { "tab-item active" } else { "tab-item" }
                                on:click={
                                    let tab_id = tab_id_for_click.clone();
                                    move |_| state_for_click.switch_tab(&tab_id)
                                }
                            >
                                <span class="tab-label">{display_name}</span>
                                {closable.then(|| {
                                    let state = state_for_close.clone();
                                    let tab_id = tab_id_for_close.clone();
                                    view! {
                                        <button
                                            class="tab-close"
                                            on:click=move |ev| {
                                                ev.stop_propagation();
                                                state.close_tab(&tab_id);
                                            }
                                        >
                                            "x"
                                        </button>
                                    }
                                })}
                            </div>
                        }
                    }
                />
            </div>
            <div class="tab-actions">
                <NewTabButton />
            </div>
        </div>
    }
}

#[component]
fn NewTabButton() -> impl IntoView {
    let state = expect_context::<AppState>();
    let show_menu = RwSignal::new(false);

    view! {
        <div class="new-tab-container">
            <button
                class="new-tab-btn"
                on:click=move |_| show_menu.update(|v| *v = !*v)
            >
                "+"
            </button>
            <Show when=move || show_menu.get()>
                {
                    let state = state.clone();
                    view! {
                        <div class="new-tab-menu">
                            <button
                                class="new-tab-option"
                                on:click=move |_| {
                                    state.open_scope_chat("main");
                                    show_menu.set(false);
                                }
                            >
                                "#main"
                            </button>
                        </div>
                    }
                }
            </Show>
        </div>
    }
}
