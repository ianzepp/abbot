use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

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
}

#[derive(Clone, Debug)]
pub struct Message {
    pub id: Uuid,
    pub op: MessageOp,
    pub sender: String,
    pub channel: String,
    pub data: MessageData,
    pub reply_to: Option<Uuid>,
    pub timestamp: SystemTime,
}

impl Message {
    pub fn new(op: MessageOp, sender: impl Into<String>, channel: impl Into<String>, data: MessageData) -> Self {
        Self {
            id: Uuid::new_v4(),
            op,
            sender: sender.into(),
            channel: channel.into(),
            data,
            reply_to: None,
            timestamp: SystemTime::now(),
        }
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

    pub fn chat(sender: impl Into<String>, channel: impl Into<String>, text: impl Into<String>) -> Message {
        Message::new(MessageOp::Chat, sender, channel, MessageData::Text(text.into()))
    }

    pub fn ok(sender: impl Into<String>, channel: impl Into<String>, data: MessageData) -> Message {
        Message::new(MessageOp::Ok, sender, channel, data)
    }

    pub fn ok_text(sender: impl Into<String>, channel: impl Into<String>, text: impl Into<String>) -> Message {
        Message::new(MessageOp::Ok, sender, channel, MessageData::Text(text.into()))
    }

    pub fn error(sender: impl Into<String>, channel: impl Into<String>, code: impl Into<String>, message: impl Into<String>) -> Message {
        Message::new(
            MessageOp::Error,
            sender,
            channel,
            MessageData::Error {
                code: code.into(),
                message: message.into(),
            },
        )
    }

    pub fn item(sender: impl Into<String>, channel: impl Into<String>, data: MessageData) -> Message {
        Message::new(MessageOp::Item, sender, channel, data)
    }

    pub fn item_text(sender: impl Into<String>, channel: impl Into<String>, text: impl Into<String>) -> Message {
        Message::new(MessageOp::Item, sender, channel, MessageData::Text(text.into()))
    }

    pub fn data(sender: impl Into<String>, channel: impl Into<String>, bytes: Vec<u8>) -> Message {
        Message::new(MessageOp::Data, sender, channel, MessageData::Bytes(bytes))
    }

    pub fn progress(sender: impl Into<String>, channel: impl Into<String>, percent: f32, current: u64, total: u64) -> Message {
        Message::new(
            MessageOp::Progress,
            sender,
            channel,
            MessageData::Progress { percent, current, total },
        )
    }

    pub fn event(sender: impl Into<String>, channel: impl Into<String>, kind: impl Into<String>, payload: serde_json::Value) -> Message {
        Message::new(
            MessageOp::Event,
            sender,
            channel,
            MessageData::Event {
                kind: kind.into(),
                payload,
            },
        )
    }

    pub fn done(sender: impl Into<String>, channel: impl Into<String>) -> Message {
        Message::new(MessageOp::Done, sender, channel, MessageData::Empty)
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
        assert_eq!(msg.channel, "#general");
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
