// Tab bar for switching between views.

use leptos::prelude::*;

use crate::state::AppState;

#[component]
pub fn TabBar() -> impl IntoView {
    let state = expect_context::<AppState>();

    view! {
        <div class="tab-bar">
            <For
                each=move || state.tabs.get()
                key=|tab| tab.id.clone()
                children=move |tab| {
                    let tab_id = tab.id.clone();
                    let tab_id_click = tab.id.clone();
                    let is_active = move || state.active_tab.get() == tab_id;

                    view! {
                        <button
                            class=move || if is_active() { "tab active" } else { "tab" }
                            on:click=move |_| state.active_tab.set(tab_id_click.clone())
                        >
                            {tab.title.clone()}
                        </button>
                    }
                }
            />
        </div>
    }
}
