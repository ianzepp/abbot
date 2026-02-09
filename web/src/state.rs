// Application state using Leptos signals.
//
// State is derived from the kernel frame stream. The main view shows
// Overwatch (monitoring) or room chat views selected via bottom tabs.

use leptos::prelude::*;
use std::collections::{HashMap, HashSet};

use crate::bus::Frame;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ActiveView {
    #[default]
    Monitor,
    RoomChat(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoomChatMessage {
    pub id: String,
    pub role: RoomChatRole,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RoomChatRole {
    User,
    Assistant,
}

#[derive(Clone, Default, Debug)]
pub struct RoomChatData {
    pub messages: Vec<RoomChatMessage>,
    pub selected_user_msg: Option<String>,
    pub active_thread_id: Option<String>,
    pub streaming: bool,
    pub streaming_content: String,
}

impl RoomChatData {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            selected_user_msg: None,
            active_thread_id: None,
            streaming: false,
            streaming_content: String::new(),
        }
    }
}

/// Full frame detail (data + trace) loaded on demand via frame.detail request.
#[derive(Clone, Debug, Default)]
pub struct FrameDetail {
    pub id: String,
    pub data: Option<serde_json::Value>,
    pub trace: Option<serde_json::Value>,
}

const MAX_FRAMES: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabType {
    Overwatch,
    RoomChat(String),
    SessionChat(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: String,
    pub tab_type: TabType,
    pub closable: bool,
}

impl Tab {
    pub fn display_name(&self) -> String {
        match &self.tab_type {
            TabType::Overwatch => "Overwatch".into(),
            TabType::RoomChat(name) => format!("#{}", name),
            TabType::SessionChat(short) => {
                let display = if short.len() > 8 { &short[..8] } else { short };
                format!("@{}", display)
            }
        }
    }

    pub fn overwatch() -> Self {
        Self {
            id: "overwatch".into(),
            tab_type: TabType::Overwatch,
            closable: false,
        }
    }

    pub fn room_chat(room: &str) -> Self {
        Self {
            id: format!("room:{}", room),
            tab_type: TabType::RoomChat(room.into()),
            closable: true,
        }
    }

    pub fn session_chat(session_id: &str) -> Self {
        let short = if session_id.len() > 8 {
            &session_id[..8]
        } else {
            session_id
        };
        Self {
            id: format!("session:{}", session_id),
            tab_type: TabType::SessionChat(short.into()),
            closable: true,
        }
    }
}

fn default_tabs() -> Vec<Tab> {
    vec![Tab::overwatch(), Tab::room_chat("main")]
}

#[derive(Clone, Default)]
pub struct TabState {
    pub scroll_position: RwSignal<f64>,
    pub input_text: RwSignal<String>,
    pub selected_frame_id: RwSignal<Option<String>>,
}

impl TabState {
    pub fn new() -> Self {
        Self {
            scroll_position: RwSignal::new(0.0),
            input_text: RwSignal::new(String::new()),
            selected_frame_id: RwSignal::new(None),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub connected: RwSignal<bool>,
    pub tabs: RwSignal<Vec<Tab>>,
    pub active_tab: RwSignal<String>,
    pub tab_states: RwSignal<HashMap<String, TabState>>,
    pub frames: RwSignal<Vec<Frame>>,
    pub collapsed_sections: RwSignal<HashSet<String>>,
    pub frame_filter: RwSignal<Option<String>>,
    pub selected_frame: RwSignal<Option<Frame>>,
    pub selected_frame_detail: RwSignal<Option<FrameDetail>>,
    pub paused: RwSignal<bool>,
    pub dark_mode: RwSignal<bool>,
    pub tick_seq: RwSignal<Option<u64>>,
    pub active_view: RwSignal<ActiveView>,
    pub room_chats: RwSignal<HashMap<String, RoomChatData>>,
    pub show_ok_frames: RwSignal<bool>,
    pub show_req_frames: RwSignal<bool>,
    pub show_event_frames: RwSignal<bool>,
}

impl AppState {
    pub fn new() -> Self {
        let tabs = default_tabs();
        let active_tab = tabs.first().map(|t| t.id.clone()).unwrap_or_default();

        let mut tab_states = HashMap::new();
        for tab in &tabs {
            tab_states.insert(tab.id.clone(), TabState::new());
        }

        Self {
            connected: RwSignal::new(false),
            tabs: RwSignal::new(tabs),
            active_tab: RwSignal::new(active_tab),
            tab_states: RwSignal::new(tab_states),
            frames: RwSignal::new(Vec::new()),
            collapsed_sections: RwSignal::new(HashSet::new()),
            frame_filter: RwSignal::new(None),
            selected_frame: RwSignal::new(None),
            selected_frame_detail: RwSignal::new(None),
            paused: RwSignal::new(false),
            dark_mode: RwSignal::new(false),
            tick_seq: RwSignal::new(None),
            active_view: RwSignal::new(ActiveView::default()),
            room_chats: RwSignal::new(HashMap::new()),
            show_ok_frames: RwSignal::new(false),
            show_req_frames: RwSignal::new(true),
            show_event_frames: RwSignal::new(true),
        }
    }

    pub fn add_frame(&self, frame: Frame) {
        // Handle SIGTICK separately - extract seq but don't add to timeline
        if frame.summary == "SIGTICK" {
            return;
        }

        self.frames.update(|frames| {
            frames.insert(0, frame);
            if frames.len() > MAX_FRAMES {
                frames.truncate(MAX_FRAMES);
            }
        });
    }

    pub fn clear_frames(&self) {
        self.frames.set(Vec::new());
        self.selected_frame.set(None);
        self.selected_frame_detail.set(None);
    }

    pub fn select_frame(&self, frame: Option<Frame>) {
        self.selected_frame_detail.set(None);
        self.selected_frame.set(frame);
    }

    pub fn toggle_pause(&self) {
        self.paused.update(|p| *p = !*p);
    }

    pub fn toggle_dark_mode(&self) {
        self.dark_mode.update(|d| *d = !*d);
    }

    pub fn frame_count(&self) -> usize {
        self.frames.get().len()
    }

    pub fn needs_count(&self) -> usize {
        self.frames
            .get()
            .iter()
            .filter(|f| {
                f.name
                    .as_deref()
                    .map(|n| n.starts_with("need:"))
                    .unwrap_or(false)
            })
            .count()
    }

    pub fn tasks_count(&self) -> usize {
        self.frames
            .get()
            .iter()
            .filter(|f| {
                f.name
                    .as_deref()
                    .map(|n| n.starts_with("task:"))
                    .unwrap_or(false)
            })
            .count()
    }

    pub fn tools_count(&self) -> usize {
        self.frames
            .get()
            .iter()
            .filter(|f| {
                f.name
                    .as_deref()
                    .is_some_and(|n| n.starts_with("tool:") || n == "chat:tool")
            })
            .count()
    }

    pub fn toggle_section(&self, section: &str) {
        self.collapsed_sections.update(|sections| {
            if sections.contains(section) {
                sections.remove(section);
            } else {
                sections.insert(section.to_string());
            }
        });
    }

    pub fn set_frame_filter(&self, filter: Option<String>) {
        self.frame_filter.set(filter);
    }

    pub fn active_tab_info(&self) -> Option<Tab> {
        let active_id = self.active_tab.get();
        self.tabs.get().into_iter().find(|t| t.id == active_id)
    }

    pub fn open_room_chat(&self, room: &str) {
        let tab = Tab::room_chat(room);
        let tab_id = tab.id.clone();

        self.tabs.update(|tabs| {
            if !tabs.iter().any(|t| t.id == tab_id) {
                tabs.push(tab);
            }
        });

        self.tab_states.update(|states| {
            if !states.contains_key(&tab_id) {
                states.insert(tab_id.clone(), TabState::new());
            }
        });

        self.active_tab.set(tab_id);
    }

    pub fn open_session_chat(&self, session_id: &str) {
        let tab = Tab::session_chat(session_id);
        let tab_id = tab.id.clone();

        self.tabs.update(|tabs| {
            if !tabs.iter().any(|t| t.id == tab_id) {
                tabs.push(tab);
            }
        });

        self.tab_states.update(|states| {
            if !states.contains_key(&tab_id) {
                states.insert(tab_id.clone(), TabState::new());
            }
        });

        self.active_tab.set(tab_id);
    }

    pub fn close_tab(&self, tab_id: &str) {
        let tabs = self.tabs.get();
        let tab = tabs.iter().find(|t| t.id == tab_id);

        if let Some(tab) = tab {
            if !tab.closable {
                return;
            }
        }

        let current_active = self.active_tab.get();
        let current_idx = tabs.iter().position(|t| t.id == tab_id);

        self.tabs.update(|tabs| {
            tabs.retain(|t| t.id != tab_id);
        });

        self.tab_states.update(|states| {
            states.remove(tab_id);
        });

        if current_active == tab_id {
            let new_tabs = self.tabs.get();
            if let Some(idx) = current_idx {
                let new_idx = idx.saturating_sub(1).min(new_tabs.len().saturating_sub(1));
                if let Some(new_tab) = new_tabs.get(new_idx) {
                    self.active_tab.set(new_tab.id.clone());
                }
            }
        }
    }

    pub fn switch_tab(&self, tab_id: &str) {
        let tabs = self.tabs.get();
        if tabs.iter().any(|t| t.id == tab_id) {
            self.active_tab.set(tab_id.to_string());
        }
    }

    pub fn get_tab_state(&self, tab_id: &str) -> Option<TabState> {
        self.tab_states.get().get(tab_id).cloned()
    }

    pub fn get_room_chat(&self, room: &str) -> RoomChatData {
        let chats = self.room_chats.get_untracked();
        chats.get(room).cloned().unwrap_or_default()
    }

    pub fn update_room_chat<F>(&self, room: &str, f: F)
    where
        F: FnOnce(&mut RoomChatData),
    {
        self.room_chats.update(|chats| {
            let chat = chats
                .entry(room.to_string())
                .or_insert_with(RoomChatData::new);
            f(chat);
        });
    }

    // Chat streaming methods

    pub fn set_active_thread(&self, room: &str, thread_id: &str) {
        self.update_room_chat(room, |chat| {
            chat.active_thread_id = Some(thread_id.to_string());
            chat.streaming = true;
            chat.streaming_content.clear();
        });
    }

    pub fn append_delta(&self, room: &str, _thread_id: &str, content: &str) {
        self.update_room_chat(room, |chat| {
            chat.streaming_content.push_str(content);
        });
    }

    pub fn append_tool_call(&self, room: &str, _thread_id: &str, name: &str, _arguments: &str) {
        self.update_room_chat(room, |chat| {
            chat.streaming_content
                .push_str(&format!("\n[tool: {}]\n", name));
        });
    }

    pub fn mark_turn_done(&self, room: &str, _thread_id: &str) {
        self.update_room_chat(room, |chat| {
            if !chat.streaming_content.is_empty() {
                let resp_id = format!("a{}", chat.messages.len() + 1);
                chat.messages.push(RoomChatMessage {
                    id: resp_id,
                    role: RoomChatRole::Assistant,
                    content: std::mem::take(&mut chat.streaming_content),
                });
            }
            chat.streaming = false;
            chat.active_thread_id = None;
        });
    }

    pub fn mark_turn_error(&self, room: &str, _thread_id: &str, message: &str) {
        self.update_room_chat(room, |chat| {
            let err_id = format!("e{}", chat.messages.len() + 1);
            chat.messages.push(RoomChatMessage {
                id: err_id,
                role: RoomChatRole::Assistant,
                content: format!("Error: {}", message),
            });
            chat.streaming = false;
            chat.streaming_content.clear();
            chat.active_thread_id = None;
        });
    }

    pub fn set_frame_detail(&self, detail: FrameDetail) {
        self.selected_frame_detail.set(Some(detail));
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
