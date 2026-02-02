// ActivityPanel component - displays needs, wants, tasks, and conclaves.

use leptos::prelude::*;

use crate::state::{AppState, Conclave, Need, Task, Want};

#[component]
pub fn ActivityPanel() -> impl IntoView {
    let state = expect_context::<AppState>();

    let needs = state.needs;
    let wants = state.wants;
    let tasks = state.tasks;
    let conclaves = state.conclaves;
    let collapsed_sections = state.collapsed_sections;

    let state_conclaves = state.clone();
    let state_needs = state.clone();
    let state_tasks = state.clone();
    let state_wants = state.clone();

    view! {
        <div class="panel activity-panel">
            <div class="panel-header">
                <span class="panel-header-title">
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
                        <path d="M8 2C4.68629 2 2 4.68629 2 8C2 11.3137 4.68629 14 8 14C11.3137 14 14 11.3137 14 8C14 4.68629 11.3137 2 8 2ZM8 4V8L11 10"/>
                    </svg>
                    " Activity"
                </span>
            </div>

            <div class="panel-content">
                // Conclaves
                <ActivitySection
                    title="Conclaves"
                    count=move || conclaves.get().len()
                    collapsed=move || collapsed_sections.get().contains("conclaves")
                    on_toggle=move |_| state_conclaves.toggle_section("conclaves")
                    icon=view! {
                        <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style="margin-right: 4px">
                            <path d="M5 3a2 2 0 100 4 2 2 0 000-4zM11 3a2 2 0 100 4 2 2 0 000-4zM8 9a2 2 0 100 4 2 2 0 000-4z"/>
                        </svg>
                    }
                >
                    <Show
                        when=move || !conclaves.get().is_empty()
                        fallback=|| view! { <div class="empty-state">"No conclaves yet"</div> }
                    >
                        <For
                            each=move || conclaves.get()
                            key=|c| c.id.clone()
                            let:conclave
                        >
                            <ConclaveItem conclave=conclave />
                        </For>
                    </Show>
                </ActivitySection>

                // Needs
                <ActivitySection
                    title="Needs"
                    count=move || needs.get().len()
                    collapsed=move || collapsed_sections.get().contains("needs")
                    on_toggle=move |_| state_needs.toggle_section("needs")
                    icon=view! {
                        <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style="margin-right: 4px">
                            <path d="M8 1L10.5 6H14L11 9.5L12.5 15L8 11.5L3.5 15L5 9.5L2 6H5.5L8 1Z"/>
                        </svg>
                    }
                >
                    <Show
                        when=move || !needs.get().is_empty()
                        fallback=|| view! { <div class="empty-state">"No pending needs"</div> }
                    >
                        <For
                            each=move || needs.get()
                            key=|n| n.id.clone()
                            let:need
                        >
                            <NeedItem need=need />
                        </For>
                    </Show>
                </ActivitySection>

                // Tasks
                <ActivitySection
                    title="Tasks"
                    count=move || tasks.get().len()
                    collapsed=move || collapsed_sections.get().contains("tasks")
                    on_toggle=move |_| state_tasks.toggle_section("tasks")
                    icon=view! {
                        <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style="margin-right: 4px">
                            <path d="M8 2L2 8L8 14L14 8L8 2ZM8 4L12 8L8 12L4 8L8 4Z"/>
                        </svg>
                    }
                >
                    <Show
                        when=move || !tasks.get().is_empty()
                        fallback=|| view! { <div class="empty-state">"No active tasks"</div> }
                    >
                        <For
                            each=move || tasks.get()
                            key=|t| t.id.clone()
                            let:task
                        >
                            <TaskItem task=task />
                        </For>
                    </Show>
                </ActivitySection>

                // Wants
                <ActivitySection
                    title="Wants"
                    count=move || wants.get().len()
                    collapsed=move || collapsed_sections.get().contains("wants")
                    on_toggle=move |_| state_wants.toggle_section("wants")
                    icon=view! {
                        <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style="margin-right: 4px">
                            <path d="M8 14C4.68629 14 2 11.3137 2 8C2 4.68629 4.68629 2 8 2C11.3137 2 14 4.68629 14 8C14 11.3137 11.3137 14 8 14ZM8 12C10.2091 12 12 10.2091 12 8C12 5.79086 10.2091 4 8 4C5.79086 4 4 5.79086 4 8C4 10.2091 5.79086 12 8 12ZM8 10C6.89543 10 6 9.10457 6 8C6 6.89543 6.89543 6 8 6C9.10457 6 10 6.89543 10 8C10 9.10457 9.10457 10 8 10Z"/>
                        </svg>
                    }
                >
                    <Show
                        when=move || !wants.get().is_empty()
                        fallback=|| view! { <div class="empty-state">"No wants in pool"</div> }
                    >
                        <For
                            each=move || wants.get()
                            key=|w| w.id.clone()
                            let:want
                        >
                            <WantItem want=want />
                        </For>
                    </Show>
                </ActivitySection>
            </div>
        </div>
    }
}

#[component]
fn ActivitySection<F, G>(
    title: &'static str,
    count: F,
    collapsed: G,
    on_toggle: impl Fn(()) + 'static + Send + Sync,
    icon: impl IntoView,
    children: Children,
) -> impl IntoView
where
    F: Fn() -> usize + 'static + Send + Sync + Copy,
    G: Fn() -> bool + 'static + Send + Sync + Copy,
{
    let rendered_children = children();

    view! {
        <div class="activity-section">
            <div class="activity-section-header" on:click=move |_| on_toggle(())>
                <span class="activity-section-title">
                    <span style=move || format!(
                        "display: inline-flex; transition: transform 0.1s; transform: rotate({}deg)",
                        if collapsed() { -90 } else { 0 }
                    )>
                        <svg width="10" height="10" viewBox="0 0 16 16" fill="var(--text-muted)">
                            <path d="M4.5 5.5L8 9L11.5 5.5" stroke="currentColor" stroke-width="1.5" fill="none"/>
                        </svg>
                    </span>
                    {icon}
                    {title}
                </span>
                <Show when=move || { count() > 0 }>
                    <span class="activity-section-count">{move || count()}</span>
                </Show>
            </div>
            <div
                class="activity-section-content"
                style=move || if collapsed() { "display: none" } else { "" }
            >
                {rendered_children}
            </div>
        </div>
    }
}

#[component]
fn NeedItem(need: Need) -> impl IntoView {
    let short_id = need.id.chars().take(8).collect::<String>();

    view! {
        <div class="activity-item">
            <div class="activity-item-header">
                <span class=format!("activity-item-priority {}", need.priority)></span>
                <span class="activity-item-id">{short_id}</span>
                <span class="activity-item-status pending">"pending"</span>
            </div>
            <div class="activity-item-text">{need.need}</div>
        </div>
    }
}

#[component]
fn WantItem(want: Want) -> impl IntoView {
    let short_id = want.id.chars().take(8).collect::<String>();

    view! {
        <div class="activity-item">
            <div class="activity-item-header">
                <span class=format!("activity-item-priority {}", want.priority)></span>
                <span class="activity-item-id">{short_id}</span>
            </div>
            <div class="activity-item-text">{want.want}</div>
        </div>
    }
}

#[component]
fn TaskItem(task: Task) -> impl IntoView {
    let short_id = task.id.chars().take(8).collect::<String>();

    view! {
        <div class="activity-item">
            <div class="activity-item-header">
                <span class="activity-item-id">{short_id}</span>
                <span style="font-size: 10px; color: var(--text-muted)">{task.head_id.clone()}</span>
            </div>
            <div class="activity-item-text">{task.goal}</div>
        </div>
    }
}

fn format_time(timestamp: u64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(timestamp as f64));
    let hours = date.get_hours();
    let minutes = date.get_minutes();
    format!("{:02}:{:02}", hours, minutes)
}

#[component]
fn ConclaveItem(conclave: Conclave) -> impl IntoView {
    let state = expect_context::<AppState>();
    let conclave_id = conclave.id.clone();
    let short_id = conclave
        .id
        .strip_prefix("conclave:")
        .unwrap_or(&conclave.id)
        .to_string();

    view! {
        <div
            class="activity-item clickable"
            on:click=move |_| state.open_conclave(&conclave_id)
        >
            <div class="activity-item-header">
                <span class=format!("activity-item-status {}", conclave.status)>
                    {conclave.status.clone()}
                </span>
                <span class="activity-item-time">
                    {format_time(conclave.created_at)}
                </span>
            </div>
            <div class="activity-item-text">{short_id}</div>
        </div>
    }
}
