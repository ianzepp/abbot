// FileViewer component - displays file contents with line numbers.

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::api::get_file_content;

#[component]
pub fn FileViewer(path: String) -> impl IntoView {
    let (content, set_content) = signal::<Option<Result<String, String>>>(None);

    let path_clone = path.clone();
    Effect::new(move |_| {
        let path = path_clone.clone();
        spawn_local(async move {
            match get_file_content(&path).await {
                Ok(text) => set_content.set(Some(Ok(text))),
                Err(e) => set_content.set(Some(Err(e.to_string()))),
            }
        });
    });

    view! {
        <div class="file-viewer">
            {move || {
                match content.get() {
                    None => view! {
                        <div class="loading">
                            <div class="loading-spinner"></div>
                        </div>
                    }.into_any(),
                    Some(Err(e)) => view! {
                        <div class="file-viewer-error">{e}</div>
                    }.into_any(),
                    Some(Ok(text)) => {
                        let lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
                        view! {
                            <div class="file-viewer-content">
                                <div class="file-viewer-gutter">
                                    {lines.iter().enumerate().map(|(i, _)| {
                                        view! {
                                            <div class="file-viewer-line-number">{i + 1}</div>
                                        }
                                    }).collect_view()}
                                </div>
                                <pre class="file-viewer-code">
                                    {lines.into_iter().map(|line| {
                                        let display = if line.is_empty() { " ".to_string() } else { line };
                                        view! {
                                            <div class="file-viewer-line">{display}</div>
                                        }
                                    }).collect_view()}
                                </pre>
                            </div>
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}
