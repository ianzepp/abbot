use uuid::Uuid;

/// Context for the need currently being processed.
///
/// Simplified after room refactor: LLM transcript, external tool handling,
/// and resume state are now managed by the room runner and Door.
#[derive(Debug, Clone)]
pub(super) struct ActiveNeed {
    pub(super) need_id: String,
    pub(super) need_text: String,
    pub(super) context: String,
    pub(super) scope: Option<String>,
    pub(super) reply_to: Option<Uuid>,
}
