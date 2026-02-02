// StatusBar component - displays system status and connection state.

use leptos::prelude::*;

use crate::state::{AppState, StatusBarData};

fn format_bytes(bytes: u32) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    }
}

struct SectionConfig {
    id: &'static str,
    label: &'static str,
    render: fn(&StatusBarData) -> String,
}

const SECTIONS: &[SectionConfig] = &[
    SectionConfig {
        id: "tick",
        label: "Tick",
        render: |d| format!("Tick: {}", d.tick),
    },
    SectionConfig {
        id: "queues",
        label: "Queues (N/T/W)",
        render: |d| {
            format!(
                "N:{} T:{} W:{}",
                d.needs_count, d.tasks_count, d.wants_count
            )
        },
    },
    SectionConfig {
        id: "hands",
        label: "Hands",
        render: |d| format!("Hands: {}/{}", d.hands_running, d.hands_total),
    },
    SectionConfig {
        id: "heads",
        label: "Heads",
        render: |d| format!("Heads: {}/{}", d.heads_busy, d.heads_total),
    },
    SectionConfig {
        id: "conclave",
        label: "Conclave",
        render: |d| {
            format!(
                "Next: {}s ({} total)",
                d.next_conclave_secs, d.conclaves_count
            )
        },
    },
    SectionConfig {
        id: "memory",
        label: "Memory (Self/LTM)",
        render: |d| {
            format!(
                "Self: {} | LTM: {}",
                format_bytes(d.self_bytes),
                format_bytes(d.ltm_bytes)
            )
        },
    },
];

#[component]
pub fn StatusBar() -> impl IntoView {
    let state = expect_context::<AppState>();
    let (menu_open, set_menu_open) = signal(false);

    let data = state.status_bar;
    let visible_sections = state.statusbar_sections;
    let connected = state.connected;

    view! {
        <div class="statusbar">
            <div class="statusbar-left">
                <button
                    class="statusbar-menu-button"
                    on:click=move |_| set_menu_open.update(|v| *v = !*v)
                    title="Configure statusbar"
                >
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
                        <path d="M2 4h12v1.5H2V4zm0 4h12v1.5H2V8zm0 4h12v1.5H2V12z"/>
                    </svg>
                </button>

                <Show when=move || menu_open.get()>
                    <div class="statusbar-menu">
                        <div class="statusbar-menu-header">"Statusbar Sections"</div>
                        {SECTIONS.iter().map(|section| {
                            let id = section.id;
                            let label = section.label;
                            let toggle_section = state.clone();
                            view! {
                                <label class="statusbar-menu-item">
                                    <input
                                        type="checkbox"
                                        checked=move || visible_sections.get().contains(id)
                                        on:change=move |_| toggle_section.toggle_statusbar_section(id)
                                    />
                                    <span>{label}</span>
                                </label>
                            }
                        }).collect_view()}
                    </div>
                </Show>
            </div>

            <div class="statusbar-sections">
                {SECTIONS.iter().map(|section| {
                    let id = section.id;
                    let render = section.render;
                    view! {
                        <Show when=move || visible_sections.get().contains(id)>
                            <div class="statusbar-section">
                                {move || render(&data.get())}
                            </div>
                        </Show>
                    }
                }).collect_view()}
            </div>

            <div class="statusbar-right">
                <div class=move || {
                    if connected.get() {
                        "statusbar-connection connected"
                    } else {
                        "statusbar-connection disconnected"
                    }
                }>
                    <span class="statusbar-connection-dot" />
                    {move || if connected.get() { "Connected" } else { "Disconnected" }}
                </div>
            </div>
        </div>
    }
}
