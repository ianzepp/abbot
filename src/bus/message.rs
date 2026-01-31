// Message definitions for the bus system.
//
// Messages are the primary unit of communication between services. Each message
// has an operation type (MessageOp), data payload (MessageData), and metadata
// like sender, scope, and origin. The respond module provides ergonomic builders
// for constructing common message types without boilerplate.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

use super::Scope;

// Origin identifies the source of a message. Used for filtering and routing
// decisions - for example, heads only respond to messages from humans or
// system events, not from other heads or hands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    Head,   // AI head service making decisions
    Hand,   // AI hand service executing tasks
    Human,  // Human user via CLI, IRC, or TUI
    System, // Internal system events (heartbeats, etc)
}

impl Origin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Origin::Head => "head",
            Origin::Hand => "hand",
            Origin::Human => "human",
            Origin::System => "system",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "head" => Origin::Head,
            "hand" => Origin::Hand,
            "human" => Origin::Human,
            "system" => Origin::System,
            _ => Origin::System,
        }
    }
}

// MessageOp categorizes messages by their semantic purpose. Terminal operations
// signal conversation endpoints, streaming operations carry partial results, and
// domain-specific operations (Chat, Exec, Task) route to appropriate handlers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageOp {
    // Terminal operations indicate the end of an interaction
    Ok,     // Successful completion
    Error,  // Error with details in MessageData::Error
    Done,   // Stream terminator

    // Streaming operations for partial results
    Item,   // Individual item in a collection
    Data,   // Raw binary data chunk

    // Metadata operations
    Event,     // Typed event with kind and payload
    Progress,  // Progress indicator with percentage

    // Domain-specific operations
    Chat,   // Text chat message
    Exec,   // Tool execution request
    Ping,   // Heartbeat for health monitoring
    Task,   // Task lifecycle (request, assign, progress, result)
    Need,   // Need lifecycle (request, ack, fulfilled, expired)
    Sleep,  // Head requests sleep for N seconds
    Wake,   // Harness signals head to wake
    Idle,   // Harness signals system is fully idle (no pending tasks)
}

// MessageData carries the payload for a message. The variant used must
// correspond to the MessageOp - for example, MessageOp::Exec requires
// MessageData::Exec. Using serde_json::Value for Event and Json variants
// allows extensible payloads without changing the protocol.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MessageData {
    Text(String),
    Error {
        code: String,
        message: String,
    },
    Progress {
        percent: f32,
        current: u64,
        total: u64,
    },
    Event {
        kind: String,
        payload: serde_json::Value,
    },
    Bytes(Vec<u8>),
    Json(serde_json::Value),
    Empty,
    Exec {
        tool: String,
        args: String,
    },
    Ping {
        tick: u64,
        timestamp: u64,
    },
    Task(TaskMsg),
    Need(NeedMsg),
    Sleep {
        seconds: u64,
    },
    Wake {
        tick: u64,
    },
}

// NeedPriority determines dispatch order in the NeedService queue.
// Higher priority needs are serviced before lower priority ones.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NeedPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Urgent = 3,
}

impl Default for NeedPriority {
    fn default() -> Self {
        NeedPriority::Normal
    }
}

// NeedMsg represents the lifecycle of a need from Mind (or user) to Head.
// Needs are strategic directives that heads convert into goals. This mirrors
// TaskMsg but flows Mind→Head instead of Head→Hand.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NeedMsg {
    // Initial need request from Mind or system (user messages wrapped as needs)
    Request {
        need_id: String,
        source: String,         // "mind", "user", "system"
        priority: NeedPriority,
        need: String,           // What needs to happen
        context: String,        // Supporting information
    },
    // Head acknowledges it's working on this need
    Acknowledged {
        need_id: String,
        head_id: String,
    },
    // Head reports the need has been addressed
    Fulfilled {
        need_id: String,
        head_id: String,
        summary: String,
    },
    // Need expired or was cancelled
    Expired {
        need_id: String,
        reason: String,
    },
}

// TaskMsg represents the lifecycle of a task from request to completion.
// Tasks are created by heads (which define the goal) and executed by hands
// (which perform the actual work). Echo messages capture tool execution
// output for the audit trail.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TaskMsg {
    // Initial task request from a human or system
    Request {
        task_id: String,
        head_id: String,    // Which head should process this task
        goal: String,       // High-level objective
        input: String,      // Additional constraints/context
        notify_scope: Option<String>, // Where to post results
    },
    // Task assignment to a specific hand for execution
    Assigned {
        task_id: String,
        head_id: String,
        hand_id: String,    // Which hand will execute
    },
    // Tool execution output captured from the hand
    Echo {
        task_id: String,
        hand_id: String,
        tool: String,       // Tool name (bash, read, write, etc)
        content: String,    // Output content
    },
    // Progress update during long-running tasks
    Progress {
        task_id: String,
        hand_id: String,
        note: String,       // Human-readable progress description
    },
    // Final task result with success/failure and summary
    Result {
        task_id: String,
        hand_id: String,
        ok: bool,
        summary: String,    // Final output or error message
    },
}

// Message is the primary unit of communication on the bus. All messages are
// persisted to SQLite via RuntimeBus, enabling recovery and audit trails.
// The builder pattern in respond:: provides ergonomic construction.
#[derive(Clone, Debug)]
pub struct Message {
    pub id: Uuid,
    pub op: MessageOp,
    pub origin: Origin,
    pub sender: String,
    pub scope: Scope,
    pub data: MessageData,
    pub reply_to: Option<Uuid>,
    pub timestamp: SystemTime,
}

impl Message {
    pub fn new(
        op: MessageOp,
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        data: MessageData,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            op,
            origin: Origin::System,
            sender: sender.into(),
            scope: scope.into(),
            data,
            reply_to: None,
            timestamp: SystemTime::now(),
        }
    }

    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    pub fn with_reply_to(mut self, reply_to: Uuid) -> Self {
        self.reply_to = Some(reply_to);
        self
    }

    pub fn text(&self) -> Option<&str> {
        match &self.data {
            MessageData::Text(s) => Some(s),
            _ => None,
        }
    }
}

// Response builders for ergonomic message construction. These functions
// create properly formed messages for common use cases, handling the
// MessageOp/MessageData pairing internally.
pub mod respond {
    use super::*;

    pub fn chat(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        text: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Chat,
            sender,
            scope,
            MessageData::Text(text.into()),
        )
    }

    pub fn ok(sender: impl Into<String>, scope: impl Into<Scope>, data: MessageData) -> Message {
        Message::new(MessageOp::Ok, sender, scope, data)
    }

    pub fn ok_text(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        text: impl Into<String>,
    ) -> Message {
        Message::new(MessageOp::Ok, sender, scope, MessageData::Text(text.into()))
    }

    pub fn error(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Error,
            sender,
            scope,
            MessageData::Error {
                code: code.into(),
                message: message.into(),
            },
        )
    }

    pub fn item(sender: impl Into<String>, scope: impl Into<Scope>, data: MessageData) -> Message {
        Message::new(MessageOp::Item, sender, scope, data)
    }

    pub fn item_text(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        text: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Item,
            sender,
            scope,
            MessageData::Text(text.into()),
        )
    }

    pub fn data(sender: impl Into<String>, scope: impl Into<Scope>, bytes: Vec<u8>) -> Message {
        Message::new(MessageOp::Data, sender, scope, MessageData::Bytes(bytes))
    }

    pub fn progress(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        percent: f32,
        current: u64,
        total: u64,
    ) -> Message {
        Message::new(
            MessageOp::Progress,
            sender,
            scope,
            MessageData::Progress {
                percent,
                current,
                total,
            },
        )
    }

    pub fn event(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        kind: impl Into<String>,
        payload: serde_json::Value,
    ) -> Message {
        Message::new(
            MessageOp::Event,
            sender,
            scope,
            MessageData::Event {
                kind: kind.into(),
                payload,
            },
        )
    }

    pub fn done(sender: impl Into<String>, scope: impl Into<Scope>) -> Message {
        Message::new(MessageOp::Done, sender, scope, MessageData::Empty)
    }

    pub fn exec(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        tool: impl Into<String>,
        args: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Exec,
            sender,
            scope,
            MessageData::Exec {
                tool: tool.into(),
                args: args.into(),
            },
        )
    }

    pub fn ping(sender: impl Into<String>, scope: impl Into<Scope>, tick: u64) -> Message {
        let timestamp = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        Message::new(
            MessageOp::Ping,
            sender,
            scope,
            MessageData::Ping { tick, timestamp },
        )
    }

    pub fn task_request(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        task_id: impl Into<String>,
        head_id: impl Into<String>,
        goal: impl Into<String>,
        input: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Task,
            sender,
            scope,
            MessageData::Task(TaskMsg::Request {
                task_id: task_id.into(),
                head_id: head_id.into(),
                goal: goal.into(),
                input: input.into(),
                notify_scope: None,
            }),
        )
    }

    pub fn task_request_with_notify(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        task_id: impl Into<String>,
        head_id: impl Into<String>,
        goal: impl Into<String>,
        input: impl Into<String>,
        notify_scope: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Task,
            sender,
            scope,
            MessageData::Task(TaskMsg::Request {
                task_id: task_id.into(),
                head_id: head_id.into(),
                goal: goal.into(),
                input: input.into(),
                notify_scope: Some(notify_scope.into()),
            }),
        )
    }

    pub fn task_assigned(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        task_id: impl Into<String>,
        head_id: impl Into<String>,
        hand_id: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Task,
            sender,
            scope,
            MessageData::Task(TaskMsg::Assigned {
                task_id: task_id.into(),
                head_id: head_id.into(),
                hand_id: hand_id.into(),
            }),
        )
    }

    pub fn task_progress(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        task_id: impl Into<String>,
        hand_id: impl Into<String>,
        note: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Task,
            sender,
            scope,
            MessageData::Task(TaskMsg::Progress {
                task_id: task_id.into(),
                hand_id: hand_id.into(),
                note: note.into(),
            }),
        )
    }

    pub fn task_echo(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        task_id: impl Into<String>,
        hand_id: impl Into<String>,
        tool: impl Into<String>,
        content: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Task,
            sender,
            scope,
            MessageData::Task(TaskMsg::Echo {
                task_id: task_id.into(),
                hand_id: hand_id.into(),
                tool: tool.into(),
                content: content.into(),
            }),
        )
    }

    pub fn task_result(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        task_id: impl Into<String>,
        hand_id: impl Into<String>,
        ok: bool,
        summary: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Task,
            sender,
            scope,
            MessageData::Task(TaskMsg::Result {
                task_id: task_id.into(),
                hand_id: hand_id.into(),
                ok,
                summary: summary.into(),
            }),
        )
    }

    pub fn need_request(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        need_id: impl Into<String>,
        source: impl Into<String>,
        priority: NeedPriority,
        need: impl Into<String>,
        context: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Need,
            sender,
            scope,
            MessageData::Need(NeedMsg::Request {
                need_id: need_id.into(),
                source: source.into(),
                priority,
                need: need.into(),
                context: context.into(),
            }),
        )
    }

    pub fn need_acknowledged(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        need_id: impl Into<String>,
        head_id: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Need,
            sender,
            scope,
            MessageData::Need(NeedMsg::Acknowledged {
                need_id: need_id.into(),
                head_id: head_id.into(),
            }),
        )
    }

    pub fn need_fulfilled(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        need_id: impl Into<String>,
        head_id: impl Into<String>,
        summary: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Need,
            sender,
            scope,
            MessageData::Need(NeedMsg::Fulfilled {
                need_id: need_id.into(),
                head_id: head_id.into(),
                summary: summary.into(),
            }),
        )
    }

    pub fn need_expired(
        sender: impl Into<String>,
        scope: impl Into<Scope>,
        need_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Message {
        Message::new(
            MessageOp::Need,
            sender,
            scope,
            MessageData::Need(NeedMsg::Expired {
                need_id: need_id.into(),
                reason: reason.into(),
            }),
        )
    }

    pub fn sleep(sender: impl Into<String>, scope: impl Into<Scope>, seconds: u64) -> Message {
        Message::new(
            MessageOp::Sleep,
            sender,
            scope,
            MessageData::Sleep { seconds },
        )
    }

    pub fn wake(sender: impl Into<String>, scope: impl Into<Scope>, tick: u64) -> Message {
        Message::new(
            MessageOp::Wake,
            sender,
            scope,
            MessageData::Wake { tick },
        )
    }

    pub fn idle(sender: impl Into<String>, scope: impl Into<Scope>) -> Message {
        Message::new(MessageOp::Idle, sender, scope, MessageData::Empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_message() {
        let msg = respond::chat("alice", "#general", "hello");
        assert_eq!(msg.op, MessageOp::Chat);
        assert_eq!(msg.sender, "alice");
        assert_eq!(msg.scope.to_string(), "#general");
        assert_eq!(msg.text(), Some("hello"));
    }

    #[test]
    fn test_error_message() {
        let msg = respond::error("system", "#general", "EINVAL", "invalid argument");
        assert_eq!(msg.op, MessageOp::Error);
        match msg.data {
            MessageData::Error { code, message } => {
                assert_eq!(code, "EINVAL");
                assert_eq!(message, "invalid argument");
            }
            _ => panic!("expected error data"),
        }
    }

    #[test]
    fn test_reply_to() {
        let original = respond::chat("alice", "#general", "hello");
        let reply = respond::chat("bob", "#general", "hi").with_reply_to(original.id);
        assert_eq!(reply.reply_to, Some(original.id));
    }
}
