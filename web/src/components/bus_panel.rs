// BusPanel component - displays raw bus messages for debugging.

use leptos::prelude::*;

use crate::state::AppState;

#[component]
pub fn BusPanel() -> impl IntoView {
    let state = expect_context::<AppState>();

    let raw_messages = state.raw_bus_messages;
    let state_for_clear = state.clone();

    view! {
        <div class="bus-panel">
            <div class="bus-panel-header">
                <span class="bus-panel-count">
                    {move || format!("{} messages", raw_messages.get().len())}
                </span>
                <button
                    class="bus-panel-clear"
                    on:click=move |_| state_for_clear.clear_raw_bus_messages()
                >
                    "Clear"
                </button>
            </div>
            <div class="bus-panel-messages">
                <Show
                    when=move || !raw_messages.get().is_empty()
                    fallback=|| view! { <div class="empty-state">"No bus messages yet"</div> }
                >
                    <For
                        each={move || raw_messages.get().into_iter().enumerate().collect::<Vec<_>>()}
                        key={|(idx, _)| *idx}
                        children={move |(_, msg)| {
                            view! {
                                <div class="bus-message">
                                    {msg}
                                </div>
                            }
                        }}
                    />
                </Show>
            </div>
        </div>
    }
}
