use serde::{Deserialize, Serialize};
use std::time::UNIX_EPOCH;
use uuid::Uuid;

use crate::bus::{Message, MessageData, MessageOp, Origin, Scope};

/// Wire format for Message - fully serializable to JSONL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireMessage {
    pub id: String,
    pub op: String,
    pub origin: String,
    pub sender: String,
    pub scope: String,
    pub data: MessageData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub timestamp_ms: u64,
}

impl From<&Message> for WireMessage {
    fn from(msg: &Message) -> Self {
        let timestamp_ms = msg
            .timestamp
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Self {
            id: msg.id.to_string(),
            op: format!("{:?}", msg.op),
            origin: msg.origin.as_str().to_string(),
            sender: msg.sender.clone(),
            scope: msg.scope.to_string(),
            data: msg.data.clone(),
            reply_to: msg.reply_to.map(|id| id.to_string()),
            timestamp_ms,
        }
    }
}

impl From<Message> for WireMessage {
    fn from(msg: Message) -> Self {
        WireMessage::from(&msg)
    }
}

impl TryFrom<WireMessage> for Message {
    type Error = String;

    fn try_from(wire: WireMessage) -> Result<Self, Self::Error> {
        let id = Uuid::parse_str(&wire.id).map_err(|e| format!("invalid id: {}", e))?;
        let op = parse_op(&wire.op);
        let origin = Origin::from_str(&wire.origin);
        let scope = Scope::from(wire.scope.as_str());
        let reply_to = wire
            .reply_to
            .map(|s| Uuid::parse_str(&s))
            .transpose()
            .map_err(|e| format!("invalid reply_to: {}", e))?;
        let timestamp = UNIX_EPOCH + std::time::Duration::from_millis(wire.timestamp_ms);

        Ok(Message {
            id,
            op,
            origin,
            sender: wire.sender,
            scope,
            data: wire.data,
            reply_to,
            timestamp,
        })
    }
}

fn parse_op(s: &str) -> MessageOp {
    match s {
        "Ok" => MessageOp::Ok,
        "Error" => MessageOp::Error,
        "Done" => MessageOp::Done,
        "Item" => MessageOp::Item,
        "Data" => MessageOp::Data,
        "Event" => MessageOp::Event,
        "Progress" => MessageOp::Progress,
        "Chat" => MessageOp::Chat,
        "Exec" => MessageOp::Exec,
        "Ping" => MessageOp::Ping,
        "Task" => MessageOp::Task,
        _ => MessageOp::Chat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::respond;

    #[test]
    fn roundtrip_chat_message() {
        let msg = respond::chat("alice", "#general", "hello world");
        let wire = WireMessage::from(&msg);
        let json = serde_json::to_string(&wire).unwrap();
        let parsed: WireMessage = serde_json::from_str(&json).unwrap();
        let restored: Message = parsed.try_into().unwrap();

        assert_eq!(restored.id, msg.id);
        assert_eq!(restored.op, msg.op);
        assert_eq!(restored.sender, msg.sender);
        assert_eq!(restored.scope.to_string(), msg.scope.to_string());
        assert_eq!(restored.text(), msg.text());
    }
}
