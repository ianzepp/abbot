// Top navigation bar — Field Survey Terminal style.
//
// Left group: entity/room tabs (FRAME_INDEX, rooms)
// Right group: view mode tabs (MONITOR, CONFIG, LOGS) + theme toggle

use leptos::prelude::*;

use crate::state::{ActiveView, AppState};

#[component]
pub fn NavBar() -> impl IntoView {
    let state = expect_context::<AppState>();

    let state_toggle = state.clone();
    let state_frame_index = state.clone();
    let state_main = state.clone();
    let state_monitor_tab = state.clone();
    let state_class1 = state.clone();
    let state_class2 = state.clone();
    let state_class3 = state.clone();
    let state_class4 = state.clone();

    let on_toggle_theme = move |_| {
        state_toggle.toggle_dark_mode();
    };

    let theme_icon = move || {
        if state.dark_mode.get() { "☀" } else { "☽" }
    };

    let on_frame_index = move |_| {
        state_frame_index.active_view.set(ActiveView::Monitor);
    };

    let on_main = move |_| {
        web_sys::console::log_1(&"Clicked #MAIN".into());
        state_main.active_view.set(ActiveView::RoomChat("main".into()));
    };

    let on_monitor_tab = move |_| {
        state_monitor_tab.active_view.set(ActiveView::Monitor);
    };

    let frame_index_class = move || {
        if matches!(state_class1.active_view.get(), ActiveView::Monitor) {
            "nav-item active"
        } else {
            "nav-item"
        }
    };

    let main_class = move || {
        match state_class2.active_view.get() {
            ActiveView::RoomChat(s) if s == "main" => "nav-item active",
            _ => "nav-item",
        }
    };

    let monitor_tab_class = move || {
        if matches!(state_class3.active_view.get(), ActiveView::Monitor) {
            "nav-item active"
        } else {
            "nav-item"
        }
    };

    view! {
        <nav class="nav-bar">
            <div class="nav-group nav-group-left">
                <span class="nav-icon">"☰"</span>
                <span class=frame_index_class on:click=on_frame_index>"FRAME_INDEX"</span>
                <span class=main_class on:click=on_main>"#MAIN"</span>
            </div>
            <div class="nav-group">
                <span class="nav-icon">"⛋"</span>
                <span class=monitor_tab_class on:click=on_monitor_tab>"MONITOR"</span>
                <span class="nav-item">"EXPLORER"</span>
                <span class="nav-item">"CONFIG"</span>
                <span class="nav-item">"LOGS"</span>
                <button class="theme-toggle" on:click=on_toggle_theme>
                    {theme_icon}
                </button>
            </div>
        </nav>
    }
}
