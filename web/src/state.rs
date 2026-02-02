// Application state using Leptos signals.
//
// This replaces the Zustand store from the React frontend. All state is managed
// through Leptos reactive signals which automatically trigger re-renders when
// values change.

use std::collections::HashSet;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabType {
    Chat,
    Bus,
    Self_,
    Ltm,
    File,
    Conclave,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub tab_type: TabType,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatusBarData {
    pub tick: u64,
    pub needs_count: u32,
    pub tasks_count: u32,
    pub wants_count: u32,
    pub hands_running: u32,
    pub hands_total: u32,
    pub heads_busy: u32,
    pub heads_total: u32,
    pub next_conclave_secs: u32,
    pub self_bytes: u32,
    pub ltm_bytes: u32,
    pub conclaves_count: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    #[serde(default)]
    pub children: Option<Vec<FileEntry>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub op: String,
    pub origin: String,
    pub sender: String,
    pub scope: String,
    pub data: serde_json::Value,
    pub reply_to: Option<String>,
    pub timestamp: u64,
}

impl Message {
    pub fn text(&self) -> Option<String> {
        if let Some(text) = self.data.get("Text") {
            return text.as_str().map(|s| s.to_string());
        }
        if let Some(text) = self.data.as_str() {
            return Some(text.to_string());
        }
        None
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Need {
    pub id: String,
    pub source: String,
    pub priority: String,
    pub need: String,
    pub context: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Want {
    pub id: String,
    pub want: String,
    pub context: String,
    pub priority: String,
    pub source: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub head_id: String,
    pub goal: String,
    pub notify_scope: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeadInfo {
    pub head_id: String,
    pub state: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HandInfo {
    pub hand_id: String,
    pub state: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Conclave {
    pub id: String,
    pub status: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolActivity {
    pub text: String,
    pub ts: u64,
}

fn default_tabs() -> Vec<Tab> {
    vec![
        Tab {
            id: "chat".to_string(),
            title: "Chat".to_string(),
            tab_type: TabType::Chat,
            path: None,
        },
        Tab {
            id: "bus".to_string(),
            title: "Bus".to_string(),
            tab_type: TabType::Bus,
            path: None,
        },
        Tab {
            id: "self".to_string(),
            title: "Self".to_string(),
            tab_type: TabType::Self_,
            path: None,
        },
        Tab {
            id: "ltm".to_string(),
            title: "LTM".to_string(),
            tab_type: TabType::Ltm,
            path: None,
        },
    ]
}

fn default_statusbar_sections() -> HashSet<String> {
    ["tick", "queues", "hands", "heads", "conclave", "memory"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[derive(Clone)]
pub struct AppState {
    // Connection state
    pub connected: RwSignal<bool>,

    // Tabs
    pub tabs: RwSignal<Vec<Tab>>,
    pub active_tab: RwSignal<String>,

    // File tree
    pub files: RwSignal<Vec<FileEntry>>,
    pub selected_file: RwSignal<Option<String>>,
    pub expanded_dirs: RwSignal<HashSet<String>>,

    // Chat messages
    pub messages: RwSignal<Vec<Message>>,

    // Tool activity
    pub tool_activity: RwSignal<Option<ToolActivity>>,

    // Activity state
    pub needs: RwSignal<Vec<Need>>,
    pub wants: RwSignal<Vec<Want>>,
    pub tasks: RwSignal<Vec<Task>>,
    pub heads: RwSignal<Vec<HeadInfo>>,
    pub hands: RwSignal<Vec<HandInfo>>,
    pub conclaves: RwSignal<Vec<Conclave>>,

    // Raw bus messages for debugging
    pub raw_bus_messages: RwSignal<Vec<String>>,

    // Collapsed sections
    pub collapsed_sections: RwSignal<HashSet<String>>,

    // Statusbar
    pub statusbar_sections: RwSignal<HashSet<String>>,
    pub status_bar: RwSignal<StatusBarData>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            connected: RwSignal::new(false),
            tabs: RwSignal::new(default_tabs()),
            active_tab: RwSignal::new("chat".to_string()),
            files: RwSignal::new(Vec::new()),
            selected_file: RwSignal::new(None),
            expanded_dirs: RwSignal::new(HashSet::new()),
            messages: RwSignal::new(Vec::new()),
            tool_activity: RwSignal::new(None),
            needs: RwSignal::new(Vec::new()),
            wants: RwSignal::new(Vec::new()),
            tasks: RwSignal::new(Vec::new()),
            heads: RwSignal::new(Vec::new()),
            hands: RwSignal::new(Vec::new()),
            conclaves: RwSignal::new(Vec::new()),
            raw_bus_messages: RwSignal::new(Vec::new()),
            collapsed_sections: RwSignal::new(HashSet::new()),
            statusbar_sections: RwSignal::new(default_statusbar_sections()),
            status_bar: RwSignal::new(StatusBarData::default()),
        }
    }

    // Tab operations
    pub fn open_file(&self, path: &str, name: &str) {
        self.tabs.update(|tabs| {
            if let Some(existing) = tabs.iter().find(|t| {
                matches!(t.tab_type, TabType::File) && t.path.as_deref() == Some(path)
            }) {
                self.active_tab.set(existing.id.clone());
                return;
            }

            let new_tab = Tab {
                id: format!("file-{}", path),
                title: name.to_string(),
                tab_type: TabType::File,
                path: Some(path.to_string()),
            };
            let new_id = new_tab.id.clone();
            tabs.push(new_tab);
            self.active_tab.set(new_id);
            self.selected_file.set(Some(path.to_string()));
        });
    }

    pub fn close_tab(&self, id: &str) {
        let fixed_ids = ["chat", "bus", "self", "ltm"];
        if fixed_ids.contains(&id) {
            return;
        }

        self.tabs.update(|tabs| {
            let current_active = self.active_tab.get_untracked();
            tabs.retain(|t| t.id != id);
            if current_active == id {
                let new_active = tabs.last().map(|t| t.id.clone()).unwrap_or_else(|| "chat".to_string());
                self.active_tab.set(new_active);
            }
        });
    }

    pub fn open_conclave(&self, id: &str) {
        self.tabs.update(|tabs| {
            if let Some(existing) = tabs.iter().find(|t| {
                matches!(t.tab_type, TabType::Conclave) && t.path.as_deref() == Some(id)
            }) {
                self.active_tab.set(existing.id.clone());
                return;
            }

            let short_id = id.strip_prefix("conclave:").unwrap_or(id);
            let new_tab = Tab {
                id: format!("conclave-{}", id),
                title: format!("Conclave {}", short_id),
                tab_type: TabType::Conclave,
                path: Some(id.to_string()),
            };
            let new_id = new_tab.id.clone();
            tabs.push(new_tab);
            self.active_tab.set(new_id);
        });
    }

    // Directory operations
    pub fn toggle_dir(&self, path: &str) {
        self.expanded_dirs.update(|dirs| {
            if dirs.contains(path) {
                dirs.remove(path);
            } else {
                dirs.insert(path.to_string());
            }
        });
    }

    // Message operations
    pub fn add_message(&self, message: Message) {
        self.messages.update(|messages| {
            if messages.iter().any(|m| m.id == message.id) {
                return;
            }
            messages.push(message);
        });
    }

    // Activity updates
    pub fn update_need(&self, need_id: &str, update: Need) {
        self.needs.update(|needs| {
            if let Some(existing) = needs.iter_mut().find(|n| n.id == need_id) {
                *existing = update;
            } else {
                needs.push(update);
            }
        });
    }

    pub fn remove_need(&self, need_id: &str) {
        self.needs.update(|needs| {
            needs.retain(|n| n.id != need_id);
        });
    }

    pub fn update_want(&self, want_id: &str, update: Want) {
        self.wants.update(|wants| {
            if let Some(existing) = wants.iter_mut().find(|w| w.id == want_id) {
                *existing = update;
            } else {
                wants.push(update);
            }
        });
    }

    pub fn remove_want(&self, want_id: &str) {
        self.wants.update(|wants| {
            wants.retain(|w| w.id != want_id);
        });
    }

    pub fn update_task(&self, task_id: &str, update: Task) {
        self.tasks.update(|tasks| {
            if let Some(existing) = tasks.iter_mut().find(|t| t.id == task_id) {
                *existing = update;
            } else {
                tasks.push(update);
            }
        });
    }

    pub fn remove_task(&self, task_id: &str) {
        self.tasks.update(|tasks| {
            tasks.retain(|t| t.id != task_id);
        });
    }

    pub fn update_head(&self, head_id: &str, update: HeadInfo) {
        self.heads.update(|heads| {
            if let Some(existing) = heads.iter_mut().find(|h| h.head_id == head_id) {
                *existing = update;
            } else {
                heads.push(update);
            }
        });
    }

    pub fn update_hand(&self, hand_id: &str, update: HandInfo) {
        self.hands.update(|hands| {
            if let Some(existing) = hands.iter_mut().find(|h| h.hand_id == hand_id) {
                *existing = update;
            } else {
                hands.push(update);
            }
        });
    }

    // Raw bus messages
    pub fn add_raw_bus_message(&self, json: String) {
        self.raw_bus_messages.update(|messages| {
            messages.push(json);
            if messages.len() > 500 {
                messages.drain(0..messages.len() - 500);
            }
        });
    }

    pub fn clear_raw_bus_messages(&self) {
        self.raw_bus_messages.set(Vec::new());
    }

    // Section toggles
    pub fn toggle_section(&self, section: &str) {
        self.collapsed_sections.update(|sections| {
            if sections.contains(section) {
                sections.remove(section);
            } else {
                sections.insert(section.to_string());
            }
        });
    }

    pub fn toggle_statusbar_section(&self, section: &str) {
        self.statusbar_sections.update(|sections| {
            if sections.contains(section) {
                sections.remove(section);
            } else {
                sections.insert(section.to_string());
            }
        });
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
