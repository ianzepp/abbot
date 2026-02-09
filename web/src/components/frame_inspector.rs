// Frame inspector — right panel showing selected frame details.
//
// Features the "specimen card" with handwritten script font for the
// frame name, plus structured metadata and JSON payload display.
// Data/Trace tabs load full frame detail on demand via WebSocket.

use leptos::prelude::*;

use crate::bus::{Frame, WsOutbound, bus_send};
use crate::state::AppState;

fn format_room(room: Option<&str>) -> String {
    match room {
        Some(s) => format!("#{}", s),
        None => "-".to_string(),
    }
}

#[component]
pub fn FrameInspector() -> impl IntoView {
    let state = expect_context::<AppState>();

    view! {
        <div class="inspector">
            {move || {
                match state.selected_frame.get() {
                    Some(frame) => view! { <InspectorContent frame=frame /> }.into_any(),
                    None => view! { <InspectorEmpty /> }.into_any(),
                }
            }}
        </div>
    }
}

#[component]
fn InspectorEmpty() -> impl IntoView {
    view! {
        <div class="inspector-empty">
            "SELECT_FRAME_TO_INSPECT"
        </div>
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum InspectorTab {
    #[default]
    Overview,
    Data,
    Trace,
}

#[component]
fn InspectorContent(frame: Frame) -> impl IntoView {
    let active_tab = RwSignal::new(InspectorTab::Overview);
    let frame_id = frame.id.chars().take(8).collect::<String>().to_uppercase();
    let frame_name = frame.name.clone().unwrap_or_else(|| "unknown".into());
    let actor = frame
        .actor
        .clone()
        .unwrap_or_else(|| "-".into())
        .to_uppercase();
    let room = format_room(frame.room.as_deref()).to_uppercase();

    view! {
        <div class="inspector-content">
            <InspectorHeader frame_id=frame_id.clone() />
            <SpecimenCard name=frame_name.clone() actor=actor.clone() room=room.clone() />
            <TabRow active_tab=active_tab />
            <TabContent active_tab=active_tab frame=frame />
        </div>
    }
}

#[component]
fn TabContent(active_tab: RwSignal<InspectorTab>, frame: Frame) -> impl IntoView {
    let frame_meta = frame.clone();
    let frame_data = frame.clone();
    let frame_trace = frame.clone();

    view! {
        {move || match active_tab.get() {
            InspectorTab::Overview => view! {
                <MetadataSection frame=frame_meta.clone() />
            }.into_any(),
            InspectorTab::Data => view! {
                <DataPanel frame=frame_data.clone() />
            }.into_any(),
            InspectorTab::Trace => view! {
                <TracePanel frame=frame_trace.clone() />
            }.into_any(),
        }}
    }
}

#[component]
fn InspectorHeader(frame_id: String) -> impl IntoView {
    let now = js_sys::Date::new_0();
    let date = format!(
        "{:04}-{:02}-{:02}",
        now.get_full_year(),
        now.get_month() + 1,
        now.get_date()
    );
    let time = format!(
        "{:02}:{:02}:{:02}",
        now.get_hours(),
        now.get_minutes(),
        now.get_seconds()
    );

    view! {
        <div class="detail-header">
            <div class="detail-id">{frame_id}</div>
            <div class="detail-title">"KERNEL_FRAME_RECORD"</div>
            <div class="detail-timestamp">
                {date}<br />{time}
            </div>
        </div>
    }
}

#[component]
fn SpecimenCard(name: String, actor: String, room: String) -> impl IntoView {
    view! {
        <div class="specimen-card">
            <div class="specimen-label">"FRAME_IDENTIFIER / SYSCALL_NAME"</div>
            <div class="specimen-name">{name}</div>
            <div class="specimen-attribution">"ROOM: "{room}"  ACTOR: "{actor}</div>
        </div>
    }
}

#[component]
fn TabRow(active_tab: RwSignal<InspectorTab>) -> impl IntoView {
    let tab_class = move |tab: InspectorTab| {
        if active_tab.get() == tab {
            "panel-tab active"
        } else {
            "panel-tab"
        }
    };

    view! {
        <div class="panel-tab-row">
            <span
                class=move || tab_class(InspectorTab::Overview)
                on:click=move |_| active_tab.set(InspectorTab::Overview)
            >"OVERVIEW"</span>
            <span
                class=move || tab_class(InspectorTab::Data)
                on:click=move |_| active_tab.set(InspectorTab::Data)
            >"DATA"</span>
            <span
                class=move || tab_class(InspectorTab::Trace)
                on:click=move |_| active_tab.set(InspectorTab::Trace)
            >"TRACE"</span>
        </div>
    }
}

#[component]
fn DataPanel(frame: Frame) -> impl IntoView {
    let state = expect_context::<AppState>();
    let frame_id = frame.id.clone();

    // Request full frame detail on mount
    let frame_id_req = frame_id.clone();
    Effect::new(move |_| {
        bus_send(&WsOutbound::FrameDetail {
            id: frame_id_req.clone(),
        });
    });

    view! {
        <div class="frame-data-section">
            <div class="frame-data-title">"FRAME_PAYLOAD"</div>
            <div class="frame-data-table">
                {move || {
                    let detail = state.selected_frame_detail.get();
                    let json = match detail {
                        Some(d) if d.id == frame_id => {
                            d.data.as_ref()
                                .map(|v| serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into()))
                                .unwrap_or_else(|| "null".into())
                        }
                        _ => "Loading...".into(),
                    };
                    view! {
                        <pre style="margin: 0; padding: 12px; font-size: 11px; white-space: pre-wrap; word-break: break-word;">
                            {json}
                        </pre>
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn TracePanel(frame: Frame) -> impl IntoView {
    let state = expect_context::<AppState>();
    let frame_id = frame.id.clone();
    let parent_id = frame.parent_id.clone();

    view! {
        <div class="trace-panel">
            <div class="frame-data-title">"FRAME_LINEAGE"</div>
            <div class="trace-chain">
                // Current frame
                <div class="trace-chain-item current">
                    <span class="trace-chain-marker">"●"</span>
                    <span class="trace-chain-id">{frame_id.chars().take(12).collect::<String>().to_uppercase()}</span>
                    <span class="trace-chain-label">"CURRENT"</span>
                </div>

                // Parent frame(s)
                {move || {
                    let frames = state.frames.get();
                    let mut chain = Vec::new();
                    let mut current_parent = parent_id.clone();

                    while let Some(pid) = &current_parent {
                        if let Some(parent_frame) = frames.iter().find(|f| &f.id == pid) {
                            let pid_display = pid.chars().take(12).collect::<String>().to_uppercase();
                            let name = parent_frame.name.clone().unwrap_or_else(|| "-".into()).to_uppercase();
                            chain.push((pid_display, name));
                            current_parent = parent_frame.parent_id.clone();
                        } else {
                            // Parent not in buffer
                            let pid_display = pid.chars().take(12).collect::<String>().to_uppercase();
                            chain.push((pid_display, "NOT_IN_BUFFER".into()));
                            break;
                        }
                    }

                    if chain.is_empty() {
                        view! {
                            <div class="trace-chain-item root">
                                <span class="trace-chain-marker">"○"</span>
                                <span class="trace-chain-label">"ROOT_FRAME"</span>
                            </div>
                        }.into_any()
                    } else {
                        chain.into_iter().map(|(id, name)| {
                            view! {
                                <div class="trace-chain-item parent">
                                    <span class="trace-chain-marker">"↑"</span>
                                    <span class="trace-chain-id">{id}</span>
                                    <span class="trace-chain-name">{name}</span>
                                </div>
                            }
                        }).collect_view().into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn MetadataSection(frame: Frame) -> impl IntoView {
    let name = frame
        .name
        .clone()
        .unwrap_or_else(|| "-".into())
        .to_uppercase();
    let op = frame.op.to_uppercase();
    let parent_id = frame
        .parent_id
        .clone()
        .map(|p| p.chars().take(12).collect::<String>().to_uppercase())
        .unwrap_or_else(|| "-".into());
    let actor = frame
        .actor
        .clone()
        .unwrap_or_else(|| "-".into())
        .to_uppercase();
    let room = format_room(frame.room.as_deref()).to_uppercase();

    view! {
        <div style="margin-top: 24px;">
            <div class="frame-data-title">{name.clone()}</div>
            <div class="kv-list">
                <div class="kv-row">
                    <span class="kv-label">"OP:"</span>
                    <span class="kv-value">{op}</span>
                </div>
                <div class="kv-row">
                    <span class="kv-label">"PARENT_ID:"</span>
                    <span class="kv-value">{parent_id}</span>
                </div>
                <div class="kv-row">
                    <span class="kv-label">"ACTOR:"</span>
                    <span class="kv-value">{actor}</span>
                </div>
                <div class="kv-row">
                    <span class="kv-label">"ROOM:"</span>
                    <span class="kv-value">{room}</span>
                </div>
                <div class="kv-row">
                    <span class="kv-label">"FRAME_ID:"</span>
                    <span class="kv-value">{frame.id.chars().take(12).collect::<String>().to_uppercase()}</span>
                </div>
                <div class="kv-row">
                    <span class="kv-label">"SUMMARY:"</span>
                    <span class="kv-value">{frame.summary.clone()}</span>
                </div>
            </div>
        </div>
    }
}
