use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

use super::Scope;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    Head,
    Hand,
    Human,
    System,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageOp {
    // Terminal (ends interaction)
    Ok,
    Error,
    Done,

    // Streaming
    Item,
    Data,

    // Metadata
    Event,
    Progress,

    // Chat
    Chat,

    // Tool execution
    Exec,

    // Heartbeat
    Ping,

    // Task orchestration
    Task,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MessageData {
    Text(String),
    Error { code: String, message: String },
    Progress { percent: f32, current: u64, total: u64 },
    Event { kind: String, payload: serde_json::Value },
    Bytes(Vec<u8>),
    Json(serde_json::Value),
    Empty,
    Exec { tool: String, args: String },
    Ping { tick: u64, timestamp: u64 },
    Task(TaskMsg),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TaskMsg {
    Request {
        task_id: String,
        head_id: String,
        goal: String,
        input: String,
    },
    Assigned {
        task_id: String,
        head_id: String,
        hand_id: String,
    },
    Echo {
        task_id: String,
        hand_id: String,
        tool: String,
        content: String,
    },
    Progress {
        task_id: String,
        hand_id: String,
        note: String,
    },
    Result {
        task_id: String,
        hand_id: String,
        ok: bool,
        summary: String,
    },
}

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
    pub fn new(op: MessageOp, sender: impl Into<String>, scope: impl Into<Scope>, data: MessageData) -> Self {
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

// Response builders
pub mod respond {
    use super::*;

    pub fn chat(sender: impl Into<String>, scope: impl Into<Scope>, text: impl Into<String>) -> Message {
        Message::new(MessageOp::Chat, sender, scope, MessageData::Text(text.into()))
    }

    pub fn ok(sender: impl Into<String>, scope: impl Into<Scope>, data: MessageData) -> Message {
        Message::new(MessageOp::Ok, sender, scope, data)
    }

    pub fn ok_text(sender: impl Into<String>, scope: impl Into<Scope>, text: impl Into<String>) -> Message {
        Message::new(MessageOp::Ok, sender, scope, MessageData::Text(text.into()))
    }

    pub fn error(sender: impl Into<String>, scope: impl Into<Scope>, code: impl Into<String>, message: impl Into<String>) -> Message {
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

    pub fn item_text(sender: impl Into<String>, scope: impl Into<Scope>, text: impl Into<String>) -> Message {
        Message::new(MessageOp::Item, sender, scope, MessageData::Text(text.into()))
    }

    pub fn data(sender: impl Into<String>, scope: impl Into<Scope>, bytes: Vec<u8>) -> Message {
        Message::new(MessageOp::Data, sender, scope, MessageData::Bytes(bytes))
    }

    pub fn progress(sender: impl Into<String>, scope: impl Into<Scope>, percent: f32, current: u64, total: u64) -> Message {
        Message::new(
            MessageOp::Progress,
            sender,
            scope,
            MessageData::Progress { percent, current, total },
        )
    }

    pub fn event(sender: impl Into<String>, scope: impl Into<Scope>, kind: impl Into<String>, payload: serde_json::Value) -> Message {
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

    pub fn exec(sender: impl Into<String>, scope: impl Into<Scope>, tool: impl Into<String>, args: impl Into<String>) -> Message {
        Message::new(MessageOp::Exec, sender, scope, MessageData::Exec { tool: tool.into(), args: args.into() })
    }

    pub fn ping(sender: impl Into<String>, scope: impl Into<Scope>, tick: u64) -> Message {
        let timestamp = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        Message::new(MessageOp::Ping, sender, scope, MessageData::Ping { tick, timestamp })
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
