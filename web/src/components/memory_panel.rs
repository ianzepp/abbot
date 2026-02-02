// MemoryPanel component - displays Self or LTM content.

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::api::{get_ltm, get_self};

#[component]
pub fn MemoryPanel(
    title: &'static str,
    description: &'static str,
    fetch_type: &'static str,
) -> impl IntoView {
    let (content, set_content) = signal::<Option<Result<String, String>>>(None);
    let (loading, set_loading) = signal(true);

    let load_content = move || {
        set_loading.set(true);
        spawn_local(async move {
            let result = if fetch_type == "self" {
                get_self().await.map(|r| r.content)
            } else {
                get_ltm().await.map(|r| r.content)
            };

            match result {
                Ok(text) => set_content.set(Some(Ok(text))),
                Err(e) => set_content.set(Some(Err(e.to_string()))),
            }
            set_loading.set(false);
        });
    };

    Effect::new(move |_| {
        load_content();
    });

    view! {
        <div class="memory-panel">
            <div class="memory-panel-header">
                <h2>{title}</h2>
                <p class="memory-panel-description">{description}</p>
                <button
                    class="memory-panel-refresh"
                    on:click=move |_| load_content()
                    disabled=move || loading.get()
                    title="Refresh"
                >
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
                        <path d="M8 3a5 5 0 104.546 2.914.75.75 0 011.366-.618A6.5 6.5 0 118 1.5a.75.75 0 010 1.5z"/>
                        <path d="M8 1.5a.75.75 0 01.75.75v3.5a.75.75 0 01-1.5 0v-3.5A.75.75 0 018 1.5z"/>
                        <path d="M10.47 2.22a.75.75 0 111.06 1.06l-2.5 2.5a.75.75 0 01-1.06-1.06l2.5-2.5z"/>
                    </svg>
                </button>
            </div>
            <div class="memory-panel-content">
                {move || {
                    if loading.get() {
                        view! { <div class="memory-panel-loading">"Loading..."</div> }.into_any()
                    } else {
                        match content.get() {
                            Some(Err(e)) => view! {
                                <div class="memory-panel-error">{e}</div>
                            }.into_any(),
                            Some(Ok(text)) if !text.is_empty() => view! {
                                <div class="memory-panel-text markdown-content">
                                    <pre>{text}</pre>
                                </div>
                            }.into_any(),
                            _ => view! {
                                <div class="memory-panel-empty">"(empty)"</div>
                            }.into_any(),
                        }
                    }
                }}
            </div>
        </div>
    }
}
