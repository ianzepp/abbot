//! Application state — rooms, messages, modes, and monitor views.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;
use tui_input::Input;

use crate::theme::Theme;

// =============================================================================
// VIEWS & MODES
// =============================================================================

/// Top-level view tabs, switchable via number keys or Ctrl-T picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum View {
    Chat,
    Monitor,
    Config,
}

/// Monitor sub-filter: which frame subset to display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Frames,
    Needs,
    Tasks,
}

/// Input mode (vi-like).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AppView {
    Chat,
    Hands,
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

// =============================================================================
// FRAME & MONITOR TYPES
// =============================================================================

/// Wire-format frame received from the daemon (defined in ws module).
pub use crate::ws::Frame;

/// A frame with local arrival timestamp and resolution state.
#[derive(Debug, Clone)]
pub struct FrameRecord {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub frame: Frame,
    pub resolved: Option<String>,
}

/// One 500ms bucket of frame op counts for the monitor sparkline.
pub struct TimelineBucket {
    pub timestamp: Instant,
    pub counts: FrameCounts,
}

#[derive(Default, Clone)]
pub struct FrameCounts {
    pub req: u16,
    pub ok: u16,
    pub done: u16,
    pub error: u16,
    pub item: u16,
    pub other: u16,
}

// =============================================================================
// CHAT TYPES
// =============================================================================

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
    /// Millis timestamp of last successful replay (0 = never replayed)
    pub last_replay_ts: i64,
    /// Transient status text (e.g. "[thinking..]") shown during agent work.
    pub status_text: Option<String>,
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

// =============================================================================
// ROOT APP STATE
// =============================================================================

/// Root application state.
pub struct App {
    // Chat view state
    pub rooms: Vec<Room>,
    pub active_room: usize,
    pub active_chat_view: AppView,
    pub mode: Mode,
    pub input: Input,
    pub input_pending: bool,
    pub input_pending_room: String,
    pub show_activity: bool,
    pub cwd: String,
    pub farewell_text: Option<String>,
    pub hand_log: Vec<HandLogEntry>,

    // Global state
    pub view: View,
    pub connected: bool,
    pub theme: Theme,

    // Monitor view state
    pub frames: VecDeque<FrameRecord>,
    pub monitor_pending: HashMap<uuid::Uuid, usize>,
    pub timeline: VecDeque<TimelineBucket>,
    pub syscall_ticker: VecDeque<String>,
    pub view_mode: ViewMode,
    pub paused: bool,
    pub selected: usize,
    pub need_count: usize,
    pub task_count: usize,
    pub tool_count: usize,
    pub reply_count: usize,
    pub tick_count: usize,
    pub show_detail: bool,
    pub show_view_picker: bool,
    pub view_picker_selected: usize,
    pub queued_count: usize,
}

// =============================================================================
// IMPLEMENTATIONS
// =============================================================================

impl App {
    pub fn new(initial_room: &str, dark_mode: bool) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "?".into());
        Self {
            // Chat
            rooms: vec![Room::new(initial_room)],
            active_room: 0,
            active_chat_view: AppView::Chat,
            mode: Mode::Insert,
            input: Input::default(),
            input_pending: false,
            input_pending_room: String::new(),
            show_activity: true,
            cwd,
            farewell_text: None,
            hand_log: Vec::new(),

            // Global
            view: View::Chat,
            connected: false,
            theme: Theme::for_mode(dark_mode),

            // Monitor
            frames: VecDeque::with_capacity(1000),
            monitor_pending: HashMap::new(),
            timeline: VecDeque::with_capacity(120),
            syscall_ticker: VecDeque::with_capacity(50),
            view_mode: ViewMode::Frames,
            paused: false,
            selected: 0,
            need_count: 0,
            task_count: 0,
            tool_count: 0,
            reply_count: 0,
            tick_count: 0,
            show_detail: false,
            show_view_picker: false,
            view_picker_selected: 0,
            queued_count: 0,
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

    /// Push a frame into the monitor view, updating counters and pending map.
    pub fn push_frame(&mut self, frame: Frame) {
        let is_tick = frame.op == "event"
            && frame
                .data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(|k| k.as_str())
                == Some("SIGTICK");

        if is_tick {
            if let Some(seq) = frame
                .data
                .as_ref()
                .and_then(|d| d.get("seq"))
                .and_then(|s| s.as_u64())
            {
                self.tick_count = seq as usize;
            }
            self.advance_timeline();
            return;
        }

        if matches!(frame.op.as_str(), "ok" | "done" | "error") {
            if let Some(parent_id) = &frame.parent_id {
                if let Some(&idx) = self.monitor_pending.get(parent_id)
                    && let Some(rec) = self.frames.get_mut(idx)
                {
                    rec.resolved = Some(frame.op.clone());
                }
                self.monitor_pending.remove(parent_id);
            }
            return;
        }

        let now = chrono::Local::now();

        if let Some(name) = &frame.name {
            if name.starts_with("need:") {
                self.need_count += 1;
            } else if name.starts_with("task:") {
                self.task_count += 1;
            } else if name.starts_with("tool:") || name == "chat:tool" {
                self.tool_count += 1;
            } else if name.starts_with("reply:") {
                self.reply_count += 1;
            }
        }

        let idx = self.frames.len();
        if frame.op == "req" {
            self.monitor_pending.insert(frame.id, idx);
        }

        let op = frame.op.clone();
        self.frames.push_back(FrameRecord {
            timestamp: now,
            frame,
            resolved: None,
        });

        if self.frames.len() > 1000 {
            self.frames.pop_front();
            self.monitor_pending.retain(|_, v| *v > 0);
            for v in self.monitor_pending.values_mut() {
                *v = v.saturating_sub(1);
            }
        }

        self.update_timeline(&op);

        if let Some(rec) = self.frames.back() {
            let label = rec.frame.name.as_deref().unwrap_or(&rec.frame.op);
            self.syscall_ticker.push_back(label.to_string());
            while self.syscall_ticker.len() > 50 {
                self.syscall_ticker.pop_front();
            }
        }
    }

    fn advance_timeline(&mut self) {
        let now = Instant::now();
        let bucket_duration = std::time::Duration::from_millis(500);

        if self.timeline.is_empty()
            || now.duration_since(self.timeline.back().unwrap().timestamp) >= bucket_duration
        {
            self.timeline.push_back(TimelineBucket {
                timestamp: now,
                counts: FrameCounts::default(),
            });
        }

        while self.timeline.len() > 120 {
            self.timeline.pop_front();
        }
    }

    fn update_timeline(&mut self, op: &str) {
        if let Some(bucket) = self.timeline.back_mut() {
            match op {
                "req" => bucket.counts.req += 1,
                "ok" => bucket.counts.ok += 1,
                "done" => bucket.counts.done += 1,
                "error" => bucket.counts.error += 1,
                "item" | "progress" => bucket.counts.item += 1,
                _ => bucket.counts.other += 1,
            }
        }
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
