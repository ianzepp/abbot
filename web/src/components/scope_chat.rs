// Scope chat — journal-style two-page chat interface.
//
// Left page: FIELD_NOTES (user messages)
// Right page: ANALYSIS_LOG (assistant responses, tool calls)
// Clicking a user message shows its responses on the right.
// Clicking a tool call opens a sticky note overlay with details.

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, Response};

use crate::state::{AppState, ScopeChatMessage, ScopeChatRole};

fn user_messages(messages: &[ScopeChatMessage]) -> Vec<ScopeChatMessage> {
    messages.iter()
        .filter(|m| m.role == ScopeChatRole::User)
        .cloned()
        .collect()
}

fn responses_for(messages: &[ScopeChatMessage], user_msg_id: &str) -> Vec<ScopeChatMessage> {
    let mut collecting = false;
    let mut responses = Vec::new();

    for msg in messages {
        if msg.role == ScopeChatRole::User {
            if msg.id == user_msg_id {
                collecting = true;
                continue;
            } else if collecting {
                break;
            }
        }
        if collecting {
            responses.push(msg.clone());
        }
    }
    responses
}

fn api_messages(messages: &[ScopeChatMessage]) -> Vec<serde_json::Value> {
    messages.iter()
        .map(|m| {
            serde_json::json!({
                "role": match m.role {
                    ScopeChatRole::User => "user",
                    ScopeChatRole::Assistant => "assistant",
                },
                "content": m.content
            })
        })
        .collect()
}

async fn send_completion(messages: Vec<serde_json::Value>) -> Result<String, String> {
    let window = web_sys::window().ok_or("No window")?;
    let location = window.location();
    let origin = location.origin().map_err(|_| "No origin")?;
    let url = format!("{}/v1/chat/completions", origin);

    let body = serde_json::json!({
        "model": "default",
        "messages": messages
    });
    let body_str = serde_json::to_string(&body).map_err(|e| e.to_string())?;

    let opts = RequestInit::new();
    opts.set_method("POST");
    opts.set_body(&wasm_bindgen::JsValue::from_str(&body_str));

    let request = Request::new_with_str_and_init(&url, &opts).map_err(|e| format!("{:?}", e))?;
    request
        .headers()
        .set("Content-Type", "application/json")
        .map_err(|e| format!("{:?}", e))?;

    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("{:?}", e))?;

    let resp: Response = resp_value
        .dyn_into()
        .map_err(|_| "Response cast failed")?;

    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let text = JsFuture::from(resp.text().map_err(|e| format!("{:?}", e))?)
        .await
        .map_err(|e| format!("{:?}", e))?;

    let text_str = text.as_string().ok_or("Response not string")?;

    // Parse OpenAI-style response
    let json: serde_json::Value = serde_json::from_str(&text_str).map_err(|e| e.to_string())?;
    let content = json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("No response")
        .to_string();

    Ok(content)
}


#[component]
pub fn ScopeChat(scope: String) -> impl IntoView {
    let app_state = expect_context::<AppState>();
    let scope_for_submit = scope.clone();
    let scope_for_notes = scope.clone();
    let scope_for_log = scope.clone();

    let input_text = RwSignal::new(String::new());
    let loading = RwSignal::new(false);

    let on_input = move |ev| {
        let value = event_target_value(&ev);
        input_text.set(value);
    };

    let app_state_submit = app_state.clone();
    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            let text = input_text.get();
            if text.trim().is_empty() || loading.get() {
                return;
            }

            let chat_data = app_state_submit.get_scope_chat(&scope_for_submit);
            let new_id = format!("u{}", chat_data.messages.len() + 1);
            let new_msg = ScopeChatMessage {
                id: new_id.clone(),
                role: ScopeChatRole::User,
                content: text,
            };

            let scope_update = scope_for_submit.clone();
            app_state_submit.update_scope_chat(&scope_update, |chat| {
                chat.messages.push(new_msg);
                chat.selected_user_msg = Some(new_id.clone());
            });
            input_text.set(String::new());

            // Build messages for API
            let updated_chat = app_state_submit.get_scope_chat(&scope_for_submit);
            let api_msgs = api_messages(&updated_chat.messages);
            let app_state_response = app_state_submit.clone();
            let scope_response = scope_for_submit.clone();

            loading.set(true);

            leptos::task::spawn_local(async move {
                match send_completion(api_msgs).await {
                    Ok(content) => {
                        let chat = app_state_response.get_scope_chat(&scope_response);
                        let resp_id = format!("a{}", chat.messages.len() + 1);
                        let resp_msg = ScopeChatMessage {
                            id: resp_id,
                            role: ScopeChatRole::Assistant,
                            content,
                        };
                        app_state_response.update_scope_chat(&scope_response, |c| {
                            c.messages.push(resp_msg);
                        });
                    }
                    Err(e) => {
                        let chat = app_state_response.get_scope_chat(&scope_response);
                        let err_id = format!("e{}", chat.messages.len() + 1);
                        let err_msg = ScopeChatMessage {
                            id: err_id,
                            role: ScopeChatRole::Assistant,
                            content: format!("Error: {}", e),
                        };
                        app_state_response.update_scope_chat(&scope_response, |c| {
                            c.messages.push(err_msg);
                        });
                    }
                }
                loading.set(false);
            });
        }
    };

    view! {
        <div class="scope-chat">
            <div class="journal-spread">
                <FieldNotes scope=scope_for_notes />
                <AnalysisLog scope=scope_for_log loading=loading />
            </div>
            <div class="journal-input">
                <textarea
                    class="journal-input-field"
                    placeholder="ENTER_FIELD_OBSERVATION..."
                    prop:value=move || input_text.get()
                    on:input=on_input
                    on:keydown=on_keydown
                    rows="1"
                ></textarea>
                <span class="journal-input-hint">"[↵] SHIFT+↵ FOR NEWLINE"</span>
            </div>
        </div>
    }
}

#[component]
fn FieldNotes(scope: String) -> impl IntoView {
    let app_state = expect_context::<AppState>();
    let scope_for_click = scope.clone();

    view! {
        <div class="journal-page field-notes">
            <div class="journal-page-header">"FIELD_NOTES"</div>
            <div class="journal-page-content">
                {move || {
                    let chat_data = app_state.scope_chats.get()
                        .get(&scope)
                        .cloned()
                        .unwrap_or_default();
                    let selected = chat_data.selected_user_msg.clone();

                    user_messages(&chat_data.messages).into_iter().map(|msg| {
                        let msg_id = msg.id.clone();
                        let is_selected = selected.as_deref() == Some(&msg_id);
                        let msg_id_click = msg.id.clone();
                        let app_state_click = app_state.clone();
                        let scope_click = scope_for_click.clone();
                        let on_click = move |_| {
                            app_state_click.update_scope_chat(&scope_click, |c| {
                                c.selected_user_msg = Some(msg_id_click.clone());
                            });
                        };
                        let row_class = if is_selected {
                            "field-note selected"
                        } else {
                            "field-note"
                        };
                        view! {
                            <div class=row_class on:click=on_click>
                                <span class="field-note-marker">">"</span>
                                <span class="field-note-text">{msg.content.clone()}</span>
                            </div>
                        }
                    }).collect_view()
                }}
            </div>
        </div>
    }
}

#[component]
fn AnalysisLog(scope: String, loading: RwSignal<bool>) -> impl IntoView {
    let app_state = expect_context::<AppState>();

    view! {
        <div class="journal-page analysis-log">
            <div class="journal-page-header">"ANALYSIS_LOG"</div>
            <div class="journal-page-content">
                {move || {
                    let chat_data = app_state.scope_chats.get()
                        .get(&scope)
                        .cloned()
                        .unwrap_or_default();

                    match chat_data.selected_user_msg {
                        None => view! {
                            <div class="analysis-empty">"SELECT_FIELD_NOTE"</div>
                        }.into_any(),
                        Some(user_id) => {
                            let responses = responses_for(&chat_data.messages, &user_id);
                            let is_loading = loading.get();
                            if responses.is_empty() && !is_loading {
                                view! {
                                    <div class="analysis-empty">"AWAITING_RESPONSE"</div>
                                }.into_any()
                            } else {
                                view! {
                                    <div>
                                        {responses.into_iter().map(|msg| {
                                            view! { <AnalysisEntry msg=msg /> }
                                        }).collect_view()}
                                        {if is_loading {
                                            Some(view! { <div class="analysis-loading">"PROCESSING..."</div> })
                                        } else {
                                            None
                                        }}
                                    </div>
                                }.into_any()
                            }
                        }
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn AnalysisEntry(msg: ScopeChatMessage) -> impl IntoView {
    view! {
        <div class="analysis-text">{msg.content}</div>
    }
}
