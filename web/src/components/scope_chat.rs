// Scope chat — journal-style two-page chat interface.
//
// Left page: FIELD_NOTES (user messages)
// Right page: ANALYSIS_LOG (assistant responses, tool calls)
// Clicking a user message shows its responses on the right.
// Chat sends via WebSocket (chat.send), responses stream via chat.delta/done/error.

use leptos::prelude::*;
use pulldown_cmark::{Parser, html::push_html};

use crate::bus::{WsOutbound, bus_send};
use crate::state::{AppState, ScopeChatMessage, ScopeChatRole};

fn md_to_html(md: &str) -> String {
    let parser = Parser::new(md);
    let mut html = String::new();
    push_html(&mut html, parser);
    html
}

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

#[component]
pub fn ScopeChat(scope: String) -> impl IntoView {
    let app_state = expect_context::<AppState>();
    let scope_for_submit = scope.clone();
    let scope_for_notes = scope.clone();
    let scope_for_log = scope.clone();

    let input_text = RwSignal::new(String::new());

    let on_input = move |ev| {
        let value = event_target_value(&ev);
        input_text.set(value);
    };

    let app_state_submit = app_state.clone();
    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            let text = input_text.get();
            if text.trim().is_empty() {
                return;
            }

            // Check if already streaming
            let chat_data = app_state_submit.get_scope_chat(&scope_for_submit);
            if chat_data.streaming {
                return;
            }

            let new_id = format!("u{}", chat_data.messages.len() + 1);
            let new_msg = ScopeChatMessage {
                id: new_id.clone(),
                role: ScopeChatRole::User,
                content: text.clone(),
            };

            let scope_update = scope_for_submit.clone();
            app_state_submit.update_scope_chat(&scope_update, |chat| {
                chat.messages.push(new_msg);
                chat.selected_user_msg = Some(new_id.clone());
            });
            input_text.set(String::new());

            // Send via WebSocket
            bus_send(&WsOutbound::ChatSend {
                scope: scope_for_submit.clone(),
                text,
                id: Some(new_id),
            });
        }
    };

    view! {
        <div class="scope-chat">
            <div class="journal-spread">
                <FieldNotes scope=scope_for_notes />
                <AnalysisLog scope=scope_for_log />
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
fn AnalysisLog(scope: String) -> impl IntoView {
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

                    let is_streaming = chat_data.streaming;
                    let streaming_content = chat_data.streaming_content.clone();

                    match chat_data.selected_user_msg {
                        None => view! {
                            <div class="analysis-empty">"SELECT_FIELD_NOTE"</div>
                        }.into_any(),
                        Some(user_id) => {
                            let responses = responses_for(&chat_data.messages, &user_id);
                            if responses.is_empty() && !is_streaming {
                                view! {
                                    <div class="analysis-empty">"AWAITING_RESPONSE"</div>
                                }.into_any()
                            } else {
                                view! {
                                    <div>
                                        {responses.into_iter().map(|msg| {
                                            view! { <AnalysisEntry msg=msg /> }
                                        }).collect_view()}
                                        {if is_streaming && !streaming_content.is_empty() {
                                            let html = md_to_html(&streaming_content);
                                            Some(view! {
                                                <div class="analysis-text streaming markdown-body" inner_html=html></div>
                                            }.into_any())
                                        } else if is_streaming {
                                            Some(view! {
                                                <div class="analysis-loading">{"PROCESSING..."}</div>
                                            }.into_any())
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
    let html = md_to_html(&msg.content);
    view! {
        <div class="analysis-text markdown-body" inner_html=html></div>
    }
}
