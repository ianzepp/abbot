// Stat strip — three-column metrics above the timeline.
//
// Column A: Hero metric (frame count)
// Column B: Key/value breakdown (needs, tasks, tools, replies)
// Column C: Connection status list

use leptos::prelude::*;

use crate::state::AppState;

#[component]
pub fn StatStrip() -> impl IntoView {
    view! {
        <div class="stat-strip">
            <StatCardHero />
            <StatCardBreakdown />
            <StatCardStatus />
        </div>
    }
}

#[component]
fn StatCardHero() -> impl IntoView {
    let state = expect_context::<AppState>();

    let frame_count = move || state.frames.get().len();
    let is_live = move || !state.paused.get();
    let buffer_pct = move || {
        let count = state.frames.get().len();
        let max = 500;
        (count as f64 / max as f64 * 100.0).min(100.0)
    };

    view! {
        <div class="stat-card">
            <div class="stat-label">
                "FRAME_ACTIVITY "
                <span class="version">
                    {move || if is_live() { "[LIVE]" } else { "[PAUSED]" }}
                </span>
            </div>
            <div class="stat-hero">{frame_count}</div>
            <div class="stat-descriptor">"FRAMES_RECEIVED"</div>
            <div class="stat-progress">
                <div class="stat-progress-fill" style:width=move || format!("{}%", buffer_pct())></div>
            </div>
            <div class="stat-status">"← BUFFER_FILL"</div>
        </div>
    }
}

#[component]
fn StatCardBreakdown() -> impl IntoView {
    let state = expect_context::<AppState>();

    let state1 = state.clone();
    let state2 = state.clone();
    let state3 = state.clone();
    let state4 = state.clone();

    let needs = move || state1.needs_count();
    let tasks = move || state2.tasks_count();
    let tools = move || state3.tools_count();
    let replies = move || {
        state4.frames.get().iter()
            .filter(|f| f.name.as_deref().map(|n| n.starts_with("reply:")).unwrap_or(false))
            .count()
    };

    view! {
        <div class="stat-card">
            <div class="stat-label">"KERNEL_STATE"</div>
            <div class="kv-list" style="margin-top: 12px;">
                <div class="kv-row">
                    <span class="kv-label">"NEEDS:"</span>
                    <span class="kv-value">{needs}</span>
                </div>
                <div class="kv-row">
                    <span class="kv-label">"TASKS:"</span>
                    <span class="kv-value">{tasks}</span>
                </div>
                <div class="kv-row">
                    <span class="kv-label">"TOOLS:"</span>
                    <span class="kv-value">{tools}</span>
                </div>
                <div class="kv-row">
                    <span class="kv-label">"REPLIES:"</span>
                    <span class="kv-value">{replies}</span>
                </div>
            </div>
        </div>
    }
}

#[component]
fn StatCardStatus() -> impl IntoView {
    let state = expect_context::<AppState>();

    let connected = move || state.connected.get();
    let frame_count = move || state.frames.get().len();

    view! {
        <div class="stat-card">
            <div class="stat-label">"CONNECTION_STATUS"</div>
            <div class="status-list" style="margin-top: 8px;">
                <div class="status-row">
                    <span class=move || if connected() { "status-dot" } else { "status-dot disconnected" }></span>
                    <span>{move || if connected() { "WEBSOCKET OPEN" } else { "WEBSOCKET CLOSED" }}</span>
                </div>
                <div class="status-row">
                    <span class="status-icon">"◻"</span>
                    <span>"FRAMES: "{frame_count}</span>
                </div>
                <div class="status-row">
                    <span class="status-icon">"◎"</span>
                    <span>"SCOPE: MAIN"</span>
                </div>
            </div>
        </div>
    }
}
