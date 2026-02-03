// Application state using Leptos signals.
//
// State is derived from the kernel frame stream. The main view shows
// Overwatch (monitoring) or scoped chat views selected via bottom tabs.

use std::collections::{HashMap, HashSet};
use leptos::prelude::*;

use crate::bus::Frame;

const MAX_FRAMES: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabType {
    Overwatch,
    ScopeChat(String),
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
            TabType::ScopeChat(name) => format!("#{}", name),
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

    pub fn scope_chat(scope: &str) -> Self {
        Self {
            id: format!("scope:{}", scope),
            tab_type: TabType::ScopeChat(scope.into()),
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
    vec![
        Tab::overwatch(),
        Tab::scope_chat("main"),
    ]
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
        }
    }

    pub fn add_frame(&self, frame: Frame) {
        self.frames.update(|frames| {
            frames.insert(0, frame);
            if frames.len() > MAX_FRAMES {
                frames.truncate(MAX_FRAMES);
            }
        });
    }

    pub fn clear_frames(&self) {
        self.frames.set(Vec::new());
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

    pub fn open_scope_chat(&self, scope: &str) {
        let tab = Tab::scope_chat(scope);
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
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
