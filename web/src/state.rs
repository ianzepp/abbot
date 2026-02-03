// Application state using Leptos signals.
//
// State is derived from the kernel frame stream. The left panel shows raw frames,
// while other panels show interpreted views of the data.

use std::collections::HashSet;
use leptos::prelude::*;

use crate::bus::Frame;

const MAX_FRAMES: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabType {
    Chat,
    Activity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub tab_type: TabType,
}

fn default_tabs() -> Vec<Tab> {
    vec![
        Tab {
            id: "chat".to_string(),
            title: "Chat".to_string(),
            tab_type: TabType::Chat,
        },
        Tab {
            id: "activity".to_string(),
            title: "Activity".to_string(),
            tab_type: TabType::Activity,
        },
    ]
}

#[derive(Clone)]
pub struct AppState {
    // Connection state
    pub connected: RwSignal<bool>,

    // Tabs
    pub tabs: RwSignal<Vec<Tab>>,
    pub active_tab: RwSignal<String>,

    // Raw frame stream (most recent first)
    pub frames: RwSignal<Vec<Frame>>,

    // Collapsed sections in UI
    pub collapsed_sections: RwSignal<HashSet<String>>,

    // Filter for frame stream
    pub frame_filter: RwSignal<Option<String>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            connected: RwSignal::new(false),
            tabs: RwSignal::new(default_tabs()),
            active_tab: RwSignal::new("chat".to_string()),
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
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
