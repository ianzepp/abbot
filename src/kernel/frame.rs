use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameOp {
    Req,
    Cancel,
    Ok,
    Error,
    Done,
    Redirect,
    Item,
    Bytes,
    Event,
    Progress,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub id: Uuid,
    pub op: FrameOp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "actor",
        alias = "scope"
    )]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Frame {
    pub fn req(name: impl Into<String>, data: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            op: FrameOp::Req,
            name: Some(name.into()),
            parent_id: None,
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(data),
        }
    }

    pub fn req_with_id(id: Uuid, name: impl Into<String>, data: Value) -> Self {
        Self {
            id,
            op: FrameOp::Req,
            name: Some(name.into()),
            parent_id: None,
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(data),
        }
    }

    pub fn ok(parent_id: Uuid, data: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            op: FrameOp::Ok,
            name: None,
            parent_id: Some(parent_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(data),
        }
    }

    pub fn done(parent_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            op: FrameOp::Done,
            name: None,
            parent_id: Some(parent_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: None,
        }
    }

    pub fn redirect(parent_id: Uuid, data: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            op: FrameOp::Redirect,
            name: None,
            parent_id: Some(parent_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(data),
        }
    }

    pub fn error(parent_id: Uuid, data: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            op: FrameOp::Error,
            name: None,
            parent_id: Some(parent_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(data),
        }
    }

    pub fn item(parent_id: Uuid, data: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            op: FrameOp::Item,
            name: None,
            parent_id: Some(parent_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(data),
        }
    }

    pub fn bytes(parent_id: Uuid, data: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            op: FrameOp::Bytes,
            name: None,
            parent_id: Some(parent_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(data),
        }
    }

    pub fn progress(parent_id: Uuid, data: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            op: FrameOp::Progress,
            name: None,
            parent_id: Some(parent_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(data),
        }
    }

    pub fn event(parent_id: Uuid, data: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            op: FrameOp::Event,
            name: None,
            parent_id: Some(parent_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(data),
        }
    }

    pub fn cancel(target_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            op: FrameOp::Cancel,
            name: None,
            parent_id: Some(target_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: None,
        }
    }

    pub fn with_actor(mut self, actor: impl Into<String>) -> Self {
        self.actor = Some(actor.into());
        self
    }

    pub fn with_scope(self, scope: impl Into<String>) -> Self {
        self.with_actor(scope)
    }

    pub fn with_deadline(mut self, ms: u64) -> Self {
        self.deadline_ms = Some(ms);
        self
    }

    pub fn with_trace(mut self, trace: Value) -> Self {
        self.trace = Some(trace);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_frame_op_serialization() {
        assert_eq!(serde_json::to_string(&FrameOp::Req).unwrap(), "\"req\"");
        assert_eq!(
            serde_json::to_string(&FrameOp::Cancel).unwrap(),
            "\"cancel\""
        );
        assert_eq!(serde_json::to_string(&FrameOp::Ok).unwrap(), "\"ok\"");
        assert_eq!(serde_json::to_string(&FrameOp::Error).unwrap(), "\"error\"");
        assert_eq!(serde_json::to_string(&FrameOp::Done).unwrap(), "\"done\"");
        assert_eq!(
            serde_json::to_string(&FrameOp::Redirect).unwrap(),
            "\"redirect\""
        );
        assert_eq!(serde_json::to_string(&FrameOp::Item).unwrap(), "\"item\"");
        assert_eq!(serde_json::to_string(&FrameOp::Bytes).unwrap(), "\"bytes\"");
        assert_eq!(
            serde_json::to_string(&FrameOp::Progress).unwrap(),
            "\"progress\""
        );
    }

    #[test]
    fn test_frame_req_creation() {
        let frame = Frame::req("fs:read", json!({"path": "/tmp/test.txt"}));
        assert_eq!(frame.op, FrameOp::Req);
        assert_eq!(frame.name, Some("fs:read".to_string()));
        assert!(frame.parent_id.is_none());
        assert!(frame.data.is_some());
    }

    #[test]
    fn test_frame_ok_creation() {
        let req_id = Uuid::new_v4();
        let frame = Frame::ok(req_id, json!({"content": "hello"}));
        assert_eq!(frame.op, FrameOp::Ok);
        assert_eq!(frame.parent_id, Some(req_id));
        assert!(frame.name.is_none());
    }

    #[test]
    fn test_frame_serialization_skips_none() {
        let frame = Frame::req("test:call", json!({}));
        let serialized = serde_json::to_string(&frame).unwrap();
        assert!(!serialized.contains("parent_id"));
        assert!(!serialized.contains("actor"));
        assert!(!serialized.contains("deadline_ms"));
        assert!(!serialized.contains("trace"));
    }

    #[test]
    fn test_frame_with_builders() {
        let frame = Frame::req("test:call", json!({}))
            .with_actor("head/abc")
            .with_deadline(5000)
            .with_trace(json!({"span": "test"}));

        assert_eq!(frame.actor, Some("head/abc".to_string()));
        assert_eq!(frame.deadline_ms, Some(5000));
        assert!(frame.trace.is_some());
    }

    #[test]
    fn test_frame_roundtrip() {
        let original = Frame::req("fs:read", json!({"path": "/test"}))
            .with_actor("hand/anonymous")
            .with_deadline(1000);

        let json = serde_json::to_string(&original).unwrap();
        let restored: Frame = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.id, original.id);
        assert_eq!(restored.op, original.op);
        assert_eq!(restored.name, original.name);
        assert_eq!(restored.actor, original.actor);
        assert_eq!(restored.deadline_ms, original.deadline_ms);
    }
}
