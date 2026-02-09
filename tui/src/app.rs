//! Application state — rooms, messages, modes.

use tui_input::Input;

use crate::theme::Theme;

/// Root application state.
pub struct App {
    pub rooms: Vec<Room>,
    pub active_room: usize,
    pub mode: Mode,
    pub input: Input,
    pub connected: bool,
    pub theme: Theme,
    pub show_activity: bool,
    pub cwd: String,
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
    /// Millis timestamp of last successful replay (0 = never replayed).
    pub last_replay_ts: i64,
}

/// A single entry in a room's transcript.
pub struct ChatEntry {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub kind: EntryKind,
    pub content: String,
    pub status: MessageStatus,
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
}

/// Input mode (vi-like).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
}

impl App {
    pub fn new(initial_room: &str, dark_mode: bool) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "?".into());
        Self {
            rooms: vec![Room::new(initial_room)],
            active_room: 0,
            mode: Mode::Insert,
            input: Input::default(),
            connected: false,
            theme: Theme::for_mode(dark_mode),
            show_activity: true,
            cwd,
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
            last_replay_ts: 0,
        }
    }

    /// Flush the streaming buffer into a completed assistant message.
    pub fn flush_stream(&mut self) {
        if !self.streaming_buf.is_empty() {
            let content = std::mem::take(&mut self.streaming_buf);
            self.messages.push(ChatEntry {
                timestamp: chrono::Local::now(),
                kind: EntryKind::Assistant,
                content,
                status: MessageStatus::None,
            });
        }
    }

    /// Returns all pending user messages that need to be sent.
    pub fn pending_messages(&self) -> Vec<(usize, &ChatEntry)> {
        self.messages
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == EntryKind::User && e.status == MessageStatus::Pending)
            .collect()
    }

    /// Mark a message at the given index as sent.
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
