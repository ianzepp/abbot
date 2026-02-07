//! EMS core object schema.
//!
//! Documents the unified `entities` table that backs all first-class EMS objects
//! (tasks, needs, wants, and any future kinds). A single table with a `kind`
//! discriminator and a `data` JSON blob replaces the previous per-kind tables.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    String,
    Integer,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpec {
    pub name: &'static str,
    pub kind: FieldKind,
    pub required: bool,
    pub indexed: bool,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectSpec {
    pub table: &'static str,
    pub description: &'static str,
    pub fields: &'static [FieldSpec],
}

pub const ENTITIES: ObjectSpec = ObjectSpec {
    table: "entities",
    description: "Unified entity table: tasks, needs, wants, and custom kinds. \
                  Fixed columns are real SQL columns; everything else lives in the `data` JSON blob.",
    fields: &[
        FieldSpec {
            name: "id",
            kind: FieldKind::String,
            required: true,
            indexed: true,
            description: "Primary key (UUID).",
        },
        FieldSpec {
            name: "kind",
            kind: FieldKind::String,
            required: true,
            indexed: true,
            description: "Entity kind discriminator (e.g. task, need, want).",
        },
        FieldSpec {
            name: "status",
            kind: FieldKind::String,
            required: true,
            indexed: true,
            description: "Lifecycle status (e.g. pending, running, completed, fulfilled, promoted).",
        },
        FieldSpec {
            name: "priority",
            kind: FieldKind::Integer,
            required: true,
            indexed: true,
            description: "Integer priority: 0=urgent, 1=high, 2=normal, 3=low.",
        },
        FieldSpec {
            name: "scope",
            kind: FieldKind::String,
            required: true,
            indexed: true,
            description: "Owning conversation/session scope.",
        },
        FieldSpec {
            name: "prompt",
            kind: FieldKind::String,
            required: true,
            indexed: false,
            description: "Primary text content (task prompt, need instruction, want description).",
        },
        FieldSpec {
            name: "created_at",
            kind: FieldKind::String,
            required: true,
            indexed: true,
            description: "RFC3339 UTC timestamp.",
        },
        FieldSpec {
            name: "updated_at",
            kind: FieldKind::String,
            required: true,
            indexed: true,
            description: "RFC3339 UTC timestamp.",
        },
        FieldSpec {
            name: "data",
            kind: FieldKind::Json,
            required: true,
            indexed: false,
            description: "JSON blob for kind-specific extras.",
        },
    ],
};

pub const CORE_OBJECTS: &[ObjectSpec] = &[ENTITIES];

pub fn get(table: &str) -> Option<&'static ObjectSpec> {
    match table {
        "entities" | "tasks" | "needs" | "wants" => Some(&ENTITIES),
        _ => None,
    }
}

// =============================================================================
// PRIORITY HELPERS
// =============================================================================

/// Convert a text priority label to an integer rank.
/// 0 = urgent (highest), 3 = low (lowest), 2 = normal (default).
pub fn priority_to_rank(priority: &str) -> i64 {
    match priority {
        "urgent" => 0,
        "high" => 1,
        "normal" => 2,
        "low" => 3,
        _ => 2,
    }
}

/// Convert an integer priority rank to a text label.
pub fn rank_to_priority(rank: i64) -> &'static str {
    match rank {
        0 => "urgent",
        1 => "high",
        2 => "normal",
        3 => "low",
        _ => "normal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_has_id_field() {
        assert!(ENTITIES.fields.iter().any(|f| f.name == "id"));
    }

    #[test]
    fn entities_has_kind_field() {
        assert!(ENTITIES.fields.iter().any(|f| f.name == "kind"));
    }

    #[test]
    fn get_returns_entities_for_known_tables() {
        assert!(get("entities").is_some());
        assert!(get("tasks").is_some());
        assert!(get("needs").is_some());
        assert!(get("wants").is_some());
        assert!(get("unknown").is_none());
    }

    #[test]
    fn priority_roundtrip() {
        for label in &["urgent", "high", "normal", "low"] {
            let rank = priority_to_rank(label);
            assert_eq!(rank_to_priority(rank), *label);
        }
    }
}
