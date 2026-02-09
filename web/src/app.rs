// Root App component — Field Survey Terminal layout.
//
// Two-zone asymmetric split: left (65%) for stats + timeline,
// right (35%) for frame inspection.

use leptos::prelude::*;

use crate::bus::use_bus;
use crate::components::{FrameInspector, FrameTimeline, NavBar, RoomChat, StatStrip, StatusBar};
use crate::state::{ActiveView, AppState};

#[component]
pub fn App() -> impl IntoView {
    let state = AppState::new();

    provide_context(state.clone());

    use_bus(state.clone());

    let container_class = move || {
        if state.dark_mode.get() {
            "app-container dark-mode"
        } else {
            "app-container"
        }
    };

    view! {
        <div class=container_class>
            <NavBar />
            <MainContent />
            <StatusBar />
        </div>
    }
}

#[component]
fn MainContent() -> impl IntoView {
    let state = expect_context::<AppState>();

    view! {
        <div class="main-container">
            {move || {
                let view = state.active_view.get();
                web_sys::console::log_1(&format!("MainContent render: {:?}", view).into());
                match view {
                    ActiveView::Monitor => view! {
                        <LeftZone />
                        <RightZone />
                    }.into_any(),
                    ActiveView::RoomChat(room) => view! {
                        <RoomChat room=room />
                    }.into_any(),
                }
            }}
        </div>
    }
}

#[component]
fn LeftZone() -> impl IntoView {
    view! {
        <div class="left-zone">
            <StatStrip />
            <FrameTimeline />
        </div>
    }
}

#[component]
fn RightZone() -> impl IntoView {
    view! {
        <div class="right-zone">
            <FrameInspector />
        </div>
    }
}
