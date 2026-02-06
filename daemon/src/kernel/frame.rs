//! Frame - The unit of communication on the wire
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Frames are the universal wire protocol for syscalls, syscall responses, and
//! turn stream emissions. Every operation is modeled as frame exchange:
//! - Syscall request: op=Req with name="<namespace>:<verb>"
//! - Syscall response: op=Ok/Item/Done/Error with parent_id=req.id
//! - Turn stream: Frames sent to (scope, reply_to) for client consumption
//!
//! The syscall refactor establishes clean semantics:
//! - `name` is always <namespace>:<verb> for Req frames
//! - `actor` indicates authorship (user, head/<id>, hand/<id>, system)
//! - chat:* syscalls emit structured items onto the turn stream
//! - Redirect is removed; external tools use chat:tool + chat:done
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Frames are self-describing: op + name + data is sufficient for routing
//! - Parent correlation: parent_id links responses to requests
//! - Actor separation: authorship (actor) is distinct from syscall identity (name)
//! - Optional fields minimize wire overhead for high-frequency operations

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// =============================================================================
// FRAME OPERATIONS
// =============================================================================

/// Frame operation type.
///
/// WHY this enum exists: Frames serve dual roles (syscall protocol + turn
/// stream). Op disambiguates: Req initiates syscalls, Ok/Item/Done/Error are
/// syscall responses, and Item/Done/Error also appear on turn streams.

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameOp {
    Req,
    Cancel,
    Ok,
    Error,
    Done,
    Item,
    Bytes,
    Event,
    Progress,
}

// =============================================================================
// FRAME STRUCTURE
// =============================================================================

/// Core frame structure for syscall and turn stream communication.
///
/// WHY these fields exist:
/// - id: Unique frame identifier for correlation and deduplication
/// - op: Distinguishes request vs response vs stream emission
/// - name: Syscall name (<namespace>:<verb>) required for Req, optional on turn stream for filtering
/// - parent_id: Links responses to originating request
/// - actor: Authorship (user, head/<id>, hand/<id>, system) - separated from name per refactor spec
/// - deadline_ms: Timeout enforcement for syscall execution
/// - trace: Observability metadata (scope, span, etc.)
/// - data: Operation-specific payload
///
/// TRADE-OFF: All optional fields use Option to minimize wire overhead for
/// high-frequency operations (e.g., streaming text deltas), at the cost of
/// field access verbosity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub id: Uuid,
    pub ts: i64,
    pub op: FrameOp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,

    /// Actor is the authorship identity (e.g. "user", "head/<id>").
    ///
    /// WHY separate from name: Syscall refactor establishes actor as frame-level
    /// authorship, not buried in data payloads. chat:message behavior is keyed
    /// by actor (user enqueues work, head emits to turn stream).
    #[serde(skip_serializing_if = "Option::is_none", rename = "actor")]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// =============================================================================
// FRAME CONSTRUCTORS
// =============================================================================

/// Frame constructors follow a consistent pattern: required fields as params,
/// optional fields via builder methods (with_actor, with_deadline, etc.).

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

impl Frame {
    /// Create a syscall request frame.
    ///
    /// WHY: Syscall requests are the entry point for all kernel operations.
    /// Name must be <namespace>:<verb> format per refactor spec.
    pub fn req(name: impl Into<String>, data: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            ts: now_ms(),
            op: FrameOp::Req,
            name: Some(name.into()),
            parent_id: None,
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(data),
        }
    }

    /// Create a syscall request with a specific ID.
    ///
    /// WHY: Idempotency and replay protection require client-controlled IDs.
    pub fn req_with_id(id: Uuid, name: impl Into<String>, data: Value) -> Self {
        Self {
            id,
            ts: now_ms(),
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
            ts: now_ms(),
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
            ts: now_ms(),
            op: FrameOp::Done,
            name: None,
            parent_id: Some(parent_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: None,
        }
    }

    pub fn error(parent_id: Uuid, data: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            ts: now_ms(),
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
            ts: now_ms(),
            op: FrameOp::Item,
            name: None,
            parent_id: Some(parent_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(data),
        }
    }

    pub fn bytes(parent_id: Uuid, data: &[u8]) -> Self {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(data);
        Self {
            id: Uuid::new_v4(),
            ts: now_ms(),
            op: FrameOp::Bytes,
            name: None,
            parent_id: Some(parent_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(Value::String(encoded)),
        }
    }

    pub fn progress(parent_id: Uuid, data: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            ts: now_ms(),
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
            ts: now_ms(),
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
            ts: now_ms(),
            op: FrameOp::Cancel,
            name: None,
            parent_id: Some(target_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: None,
        }
    }

    /// Attach actor (authorship) to frame.
    ///
    /// WHY: chat:message behavior is keyed by actor. Syscall context uses actor
    /// for permission checks (can_mutate).
    pub fn with_actor(mut self, actor: impl Into<String>) -> Self {
        self.actor = Some(actor.into());
        self
    }

    /// Attach name to response/stream frame.
    ///
    /// WHY: Turn stream frames may include name for filtering/monitoring even
    /// though clients MUST interpret payloads by op + data.type.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Attach deadline for syscall timeout enforcement.
    ///
    /// WHY: Prevents unbounded syscall execution; dispatcher enforces deadline.
    pub fn with_deadline(mut self, ms: u64) -> Self {
        self.deadline_ms = Some(ms);
        self
    }

    /// Attach trace metadata for observability.
    ///
    /// WHY: SigcallHub uses trace to tag scope for broadcast observers.
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
        assert!(frame.ts > 0);
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
        assert!(serialized.contains("\"ts\""));
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
        assert_eq!(restored.ts, original.ts);
        assert_eq!(restored.op, original.op);
        assert_eq!(restored.name, original.name);
        assert_eq!(restored.actor, original.actor);
        assert_eq!(restored.deadline_ms, original.deadline_ms);
    }
}
