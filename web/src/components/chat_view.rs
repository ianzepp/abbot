// Chat view component with scope filtering and HTTP-based message sending.

use std::collections::HashMap;
use leptos::prelude::*;
use pulldown_cmark::{Parser, Options, html};
use wasm_bindgen::JsCast;

use crate::http::send_chat_message;
use crate::state::AppState;

fn markdown_to_html(text: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(text, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

#[derive(Clone)]
struct ChatItem {
    key: String,
    role: &'static str,
    scope: String,
    text: String,
}

fn matches_scope(frame_scope: &str, target_scope: &str) -> bool {
    if target_scope == "main" {
        frame_scope == "main" || frame_scope.starts_with("web/main")
    } else if target_scope.starts_with("session/") {
        frame_scope == target_scope || frame_scope.starts_with(&format!("{}/", target_scope))
    } else {
        frame_scope == target_scope || frame_scope.starts_with(&format!("{}/", target_scope))
    }
}

#[component]
pub fn ChatView(scope: String) -> impl IntoView {
    let state = expect_context::<AppState>();
    let input_value = RwSignal::new(String::new());
    let sending = RwSignal::new(false);

    let scope_for_items = scope.clone();
    let scope_for_submit = scope.clone();
    let scope_for_keydown = scope.clone();

    let chat_items = move || {
        let frames = state.frames.get();
        let target_scope = scope_for_items.clone();

        let mut out: Vec<ChatItem> = Vec::new();
        let mut buffers: HashMap<String, String> = HashMap::new();
        let mut thread_scope: HashMap<String, String> = HashMap::new();
        let mut thread_seq: HashMap<String, usize> = HashMap::new();

        let flush = |thread_id: &str,
                     buffers: &mut HashMap<String, String>,
                     thread_scope: &mut HashMap<String, String>,
                     thread_seq: &mut HashMap<String, usize>,
                     out: &mut Vec<ChatItem>| {
            let Some(text) = buffers.remove(thread_id) else {
                return;
            };
            if text.trim().is_empty() {
                return;
            }

            let seq = thread_seq.entry(thread_id.to_string()).or_insert(0);
            *seq += 1;
            let scope = thread_scope
                .get(thread_id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());

            out.push(ChatItem {
                key: format!("assistant:{}:{}", thread_id, seq),
                role: "assistant",
                scope,
                text,
            });
        };

        for f in frames.into_iter().rev() {
            let frame_scope = f
                .actor
                .clone()
                .or_else(|| {
                    f.data
                        .as_ref()
                        .and_then(|d| d.get("scope"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "unknown".to_string());

            if !matches_scope(&frame_scope, &target_scope) {
                if f.op == "req" && f.name.as_deref() == Some("need:enqueue") {
                    let need_scope = f
                        .data
                        .as_ref()
                        .and_then(|d| d.get("scope"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    if !matches_scope(need_scope, &target_scope) {
                        continue;
                    }
                } else {
                    continue;
                }
            }

            if f.op == "req" && f.name.as_deref() == Some("need:enqueue") {
                let scope = f
                    .data
                    .as_ref()
                    .and_then(|d| d.get("scope"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let text = f
                    .data
                    .as_ref()
                    .and_then(|d| d.get("need"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !text.trim().is_empty() {
                    out.push(ChatItem {
                        key: format!("user:{}", f.id),
                        role: "user",
                        scope,
                        text,
                    });
                }
                continue;
            }

            if f.op == "bytes" && f.name.as_deref() == Some("chat:message") {
                let thread_id = f.parent_id.clone().unwrap_or_else(|| f.id.clone());
                let text = f
                    .data
                    .as_ref()
                    .and_then(|d| d.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !text.is_empty() {
                    buffers
                        .entry(thread_id.clone())
                        .or_insert_with(String::new)
                        .push_str(text);
                }
                thread_scope.insert(thread_id, frame_scope);
                continue;
            }

            if f.op == "redirect" && f.name.as_deref() == Some("tool:request") {
                if let Some(thread_id) = f.parent_id.clone() {
                    flush(
                        &thread_id,
                        &mut buffers,
                        &mut thread_scope,
                        &mut thread_seq,
                        &mut out,
                    );
                    let tool_name = f
                        .data
                        .as_ref()
                        .and_then(|d| d.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool");
                    out.push(ChatItem {
                        key: format!("assistant:{}:tool", thread_id),
                        role: "assistant",
                        scope: frame_scope,
                        text: format!("`[tool:request] {}`", tool_name),
                    });
                }
                continue;
            }

            if f.op == "done" {
                if let Some(thread_id) = f.parent_id.clone() {
                    flush(
                        &thread_id,
                        &mut buffers,
                        &mut thread_scope,
                        &mut thread_seq,
                        &mut out,
                    );
                }
                continue;
            }
        }

        let trailing = buffers.keys().cloned().collect::<Vec<_>>();
        for thread_id in trailing {
            flush(
                &thread_id,
                &mut buffers,
                &mut thread_scope,
                &mut thread_seq,
                &mut out,
            );
        }

        out
    };

    let do_send = move |scope: String, text: String| {
        if text.trim().is_empty() || sending.get() {
            return;
        }
        sending.set(true);
        input_value.set(String::new());

        wasm_bindgen_futures::spawn_local(async move {
            let _ = send_chat_message(&scope, &text).await;
            sending.set(false);
        });
    };

    let on_submit = {
        let scope = scope_for_submit.clone();
        move |ev: web_sys::SubmitEvent| {
            ev.prevent_default();
            let text = input_value.get();
            do_send(scope.clone(), text);
        }
    };

    let on_keydown = {
        let scope = scope_for_keydown.clone();
        move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Enter" && !ev.shift_key() {
                ev.prevent_default();
                let text = input_value.get();
                do_send(scope.clone(), text);
            }
        }
    };

    view! {
        <div class="chat-view">
            <div class="chat-messages">
                <For
                    each=chat_items
                    key=|item| item.key.clone()
                    children=move |item| {
                        let cls = format!("chat-message chat-message-{}", item.role);
                        let show_meta = item.scope != "main" && item.scope != "unknown";
                        let html_content = markdown_to_html(&item.text);
                        view! {
                            <div class=cls>
                                {show_meta.then(|| view! {
                                    <div class="chat-message-meta">{item.scope.clone()}</div>
                                })}
                                <div class="chat-message-text markdown-body" inner_html=html_content />
                            </div>
                        }
                    }
                />
            </div>
            <form class="chat-input-form" on:submit=on_submit>
                <input
                    type="text"
                    class="chat-input"
                    placeholder=move || if sending.get() { "Sending..." } else { "Send a message..." }
                    prop:value=move || input_value.get()
                    prop:disabled=move || sending.get()
                    on:input=move |ev| {
                        let target = ev.target().unwrap();
                        let input = target.unchecked_ref::<web_sys::HtmlInputElement>();
                        input_value.set(input.value());
                    }
                    on:keydown=on_keydown
                />
            </form>
        </div>
    }
}
