// Overwatch monitoring view with trace timeline and frame inspector.

mod inspector;
mod timeline;
mod trace_item;

use leptos::prelude::*;

use inspector::Inspector;
use timeline::TraceTimeline;

#[component]
pub fn OverwatchView() -> impl IntoView {
    let selected_frame_id = RwSignal::new(Option::<String>::None);

    view! {
        <div class="overwatch-view">
            <div class="overwatch-timeline">
                <TraceTimeline selected_frame_id=selected_frame_id />
            </div>
            <div class="overwatch-inspector">
                <Inspector selected_frame_id=selected_frame_id />
            </div>
        </div>
    }
}
