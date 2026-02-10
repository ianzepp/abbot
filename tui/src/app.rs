//! Application state — rooms, messages, modes.

use std::collections::VecDeque;
use std::time::Instant;

use tui_input::Input;

use crate::theme::Theme;

/// Root application state.
pub struct App {
    pub rooms: Vec<Room>,
    pub active_room: usize,
    pub active_view: AppView,
    pub mode: Mode,
    pub input: Input,
    pub connected: bool,
    /// Input box is locked — waiting for server echo before clearing.
    pub input_pending: bool,
    /// Room the pending input was sent to.
    pub input_pending_room: String,
    pub theme: Theme,
    pub show_activity: bool,
    pub cwd: String,
    pub farewell_text: Option<String>,
    /// Tracks in-flight farewell request for client-side accumulation.
    pub farewell_req_id: Option<uuid::Uuid>,
    /// Accumulates farewell text deltas from the LLM.
    pub farewell_buf: String,
    pub hand_log: Vec<HandLogEntry>,
    pub started_at: Instant,
    pub developer: bool,
    pub ticker: VecDeque<String>,
    pub ticker_seq: u64,
}

/// A single chat room.
pub struct Room {
    pub room: String,
    pub messages: Vec<ChatEntry>,
    pub scroll_offset: usize,
    pub unread: bool,
    pub pending: bool,
    /// Accumulates streaming text for the current assistant response.
    pub streaming_buf: String,
    /// Tool/status lines associated with the in-flight assistant response.
    pub pending_activity: Vec<String>,
    /// Sequence number for the first frame in the current assistant block.
    pub pending_seq: Option<u64>,
    /// Millis timestamp of last successful replay (0 = never replayed).
    pub last_replay_ts: i64,
    /// Transient status text (e.g. "[thinking..]") shown during agent work.
    pub status_text: Option<String>,
    /// Thread ID from chat.ack, used as reply_to for tool result routing.
    pub thread_id: Option<String>,
}

/// A single entry in a room's transcript.
pub struct ChatEntry {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub kind: EntryKind,
    pub content: String,
    pub status: MessageStatus,
    pub activity: Vec<String>,
    pub seq: Option<u64>,
}

pub struct HandLogEntry {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub actor: String,
    pub tool: Option<String>,
    pub summary: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AppView {
    Chat,
    Frames,
    Hands,
    Ems,
}

/// Message delivery status (for user messages).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MessageStatus {
    None,    // Not a user message, or status doesn't apply
    Pending, // Waiting to be sent or acknowledged
    Sent,    // Successfully delivered
    #[allow(dead_code)]
    Failed, // Failed to deliver
}

/// Entry type, controls rendering style.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    User,
    Assistant,
    Activity,
    System,
    Mind,
}

/// Input mode (vi-like).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
}

impl App {
    pub fn new(initial_room: &str, dark_mode: bool, developer: bool) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "?".into());
        Self {
            rooms: vec![Room::new(initial_room)],
            active_room: 0,
            active_view: AppView::Chat,
            mode: Mode::Normal,
            input: Input::default(),
            connected: false,
            input_pending: false,
            input_pending_room: String::new(),
            theme: Theme::for_mode(dark_mode),
            show_activity: true,
            cwd,
            farewell_text: None,
            farewell_req_id: None,
            farewell_buf: String::new(),
            hand_log: Vec::new(),
            started_at: Instant::now(),
            developer,
            ticker: VecDeque::new(),
            ticker_seq: 0,
        }
    }

    pub fn current_room(&self) -> &Room {
        &self.rooms[self.active_room]
    }

    pub fn current_room_mut(&mut self) -> &mut Room {
        &mut self.rooms[self.active_room]
    }

    /// Find or create a room for the given name, returning its index.
    pub fn ensure_room(&mut self, room: &str) -> usize {
        if let Some(i) = self.rooms.iter().position(|r| r.room == room) {
            return i;
        }
        self.rooms.push(Room::new(room));
        self.rooms.len() - 1
    }
}

impl Room {
    pub fn new(room: &str) -> Self {
        Self {
            room: room.to_string(),
            messages: Vec::new(),
            scroll_offset: 0,
            unread: false,
            pending: false,
            streaming_buf: String::new(),
            pending_activity: Vec::new(),
            pending_seq: None,
            last_replay_ts: 0,
            status_text: None,
            thread_id: None,
        }
    }

    /// Flush the streaming buffer into a completed assistant message.
    pub fn flush_stream(&mut self) {
        if self.streaming_buf.is_empty() && self.pending_activity.is_empty() {
            return;
        }
        let content = std::mem::take(&mut self.streaming_buf);
        let activity = std::mem::take(&mut self.pending_activity);
        let seq = self.pending_seq.take();
        self.messages.push(ChatEntry {
            timestamp: chrono::Local::now(),
            kind: EntryKind::Assistant,
            content,
            status: MessageStatus::None,
            activity,
            seq,
        });
    }

    /// Returns all pending user messages that need to be sent.
    #[allow(dead_code)]
    pub fn pending_messages(&self) -> Vec<(usize, &ChatEntry)> {
        self.messages
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == EntryKind::User && e.status == MessageStatus::Pending)
            .collect()
    }

    /// Mark a message at the given index as sent.
    #[allow(dead_code)]
    pub fn mark_sent(&mut self, index: usize) {
        if let Some(msg) = self.messages.get_mut(index) {
            msg.status = MessageStatus::Sent;
        }
    }

    /// Mark a message at the given index as failed.
    #[allow(dead_code)]
    pub fn mark_failed(&mut self, index: usize) {
        if let Some(msg) = self.messages.get_mut(index) {
            msg.status = MessageStatus::Failed;
        }
    }
}
