//! EMS core object schemas.
//!
//! This module documents the intended shapes of first-class EMS-backed objects
//! (tasks, needs, wants, etc.) even when EMS runs in schema-on-write mode.
//!
//! WHY: Schema-on-write enables rapid iteration, but core system objects still
//! need an explicit contract for prompts, tooling, and future migrations.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    String,
    Integer,
    Number,
    Boolean,
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

pub const TASKS: ObjectSpec = ObjectSpec {
    table: "tasks",
    description: "Operational units of work leased to hands; snapshot state lives on the row.",
    fields: &[
        FieldSpec {
            name: "id",
            kind: FieldKind::String,
            required: true,
            indexed: true,
            description: "Primary key.",
        },
        FieldSpec {
            name: "status",
            kind: FieldKind::String,
            required: true,
            indexed: true,
            description: "Lifecycle status (e.g., pending, running, completed, failed, cancelled).",
        },
        FieldSpec {
            name: "scope",
            kind: FieldKind::String,
            required: false,
            indexed: true,
            description: "Owning conversation/session scope.",
        },
        FieldSpec {
            name: "goal",
            kind: FieldKind::String,
            required: true,
            indexed: false,
            description: "Human-readable goal for the task.",
        },
        FieldSpec {
            name: "input",
            kind: FieldKind::String,
            required: false,
            indexed: false,
            description: "Task input payload (often text).",
        },
        FieldSpec {
            name: "notify_scope",
            kind: FieldKind::String,
            required: false,
            indexed: false,
            description: "Scope to notify when the task completes.",
        },
        FieldSpec {
            name: "lease_owner",
            kind: FieldKind::String,
            required: false,
            indexed: true,
            description: "Actor/service that currently owns the lease.",
        },
        FieldSpec {
            name: "leased_at",
            kind: FieldKind::String,
            required: false,
            indexed: true,
            description: "RFC3339 UTC timestamp when leased.",
        },
        FieldSpec {
            name: "started_at",
            kind: FieldKind::String,
            required: false,
            indexed: false,
            description: "RFC3339 UTC timestamp when execution began.",
        },
        FieldSpec {
            name: "completed_at",
            kind: FieldKind::String,
            required: false,
            indexed: true,
            description: "RFC3339 UTC timestamp when execution completed.",
        },
        FieldSpec {
            name: "result",
            kind: FieldKind::Json,
            required: false,
            indexed: false,
            description: "Structured result payload (JSON).",
        },
        FieldSpec {
            name: "error",
            kind: FieldKind::String,
            required: false,
            indexed: false,
            description: "Error string when status indicates failure.",
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
    ],
};

pub const NEEDS: ObjectSpec = ObjectSpec {
    table: "needs",
    description:
        "Strategic directives owned by head; may be fulfilled by creating/monitoring tasks.",
    fields: &[
        FieldSpec {
            name: "id",
            kind: FieldKind::String,
            required: true,
            indexed: true,
            description: "Primary key.",
        },
        FieldSpec {
            name: "status",
            kind: FieldKind::String,
            required: true,
            indexed: true,
            description: "Lifecycle status (e.g., pending, running, fulfilled, cancelled).",
        },
        FieldSpec {
            name: "priority",
            kind: FieldKind::String,
            required: false,
            indexed: true,
            description: "Priority label (e.g., low, normal, high, urgent).",
        },
        FieldSpec {
            name: "actor",
            kind: FieldKind::String,
            required: false,
            indexed: true,
            description: "Owning actor/scope identifier.",
        },
        FieldSpec {
            name: "instruction",
            kind: FieldKind::String,
            required: true,
            indexed: false,
            description: "Need statement / directive.",
        },
        FieldSpec {
            name: "context",
            kind: FieldKind::String,
            required: false,
            indexed: false,
            description: "Supporting context.",
        },
        FieldSpec {
            name: "fulfilled_at",
            kind: FieldKind::String,
            required: false,
            indexed: true,
            description: "RFC3339 UTC timestamp when fulfilled.",
        },
        FieldSpec {
            name: "fulfill_result",
            kind: FieldKind::Json,
            required: false,
            indexed: false,
            description: "Structured fulfillment result payload (JSON).",
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
    ],
};

pub const WANTS: ObjectSpec = ObjectSpec {
    table: "wants",
    description: "Aspirational items maintained by mind; may be promoted into needs.",
    fields: &[
        FieldSpec {
            name: "id",
            kind: FieldKind::String,
            required: true,
            indexed: true,
            description: "Primary key.",
        },
        FieldSpec {
            name: "status",
            kind: FieldKind::String,
            required: true,
            indexed: true,
            description: "Lifecycle status (e.g., pending, promoted, removed).",
        },
        FieldSpec {
            name: "priority",
            kind: FieldKind::String,
            required: false,
            indexed: true,
            description: "Priority label.",
        },
        FieldSpec {
            name: "want",
            kind: FieldKind::String,
            required: true,
            indexed: false,
            description: "Want statement.",
        },
        FieldSpec {
            name: "context",
            kind: FieldKind::String,
            required: false,
            indexed: false,
            description: "Supporting context.",
        },
        FieldSpec {
            name: "promoted_need_id",
            kind: FieldKind::String,
            required: false,
            indexed: true,
            description: "Need ID created when promoted.",
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
    ],
};

pub const CORE_OBJECTS: &[ObjectSpec] = &[TASKS, NEEDS, WANTS];

pub fn get(table: &str) -> Option<&'static ObjectSpec> {
    CORE_OBJECTS.iter().find(|o| o.table == table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_objects_have_id_field() {
        for obj in CORE_OBJECTS {
            assert!(obj.fields.iter().any(|f| f.name == "id"));
        }
    }
}
