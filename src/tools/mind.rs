use crate::llm::ToolSpec;

use serde_json::json;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::function(
            "mind__ltm_update",
            "Update the head's long-term memory (LTM).",
            json!({
                "type": "object",
                "properties": {
                    "ops": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "kind": {"type": "string", "enum": ["append", "replace", "remove"]},
                                "content": {"type": "string"},
                                "pattern": {"type": "string"}
                            },
                            "required": ["kind"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["ops"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "mind__need_create",
            "Create a strategic need for a head to address. Use this to assign immediate work.",
            json!({
                "type": "object",
                "properties": {
                    "need": {
                        "type": "string",
                        "description": "What needs to happen - the strategic directive"
                    },
                    "context": {
                        "type": "string",
                        "description": "Supporting information or reasoning"
                    },
                    "priority": {
                        "type": "string",
                        "enum": ["low", "normal", "high", "urgent"],
                        "description": "Priority level (default: normal)"
                    }
                },
                "required": ["need"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "mind__want_list",
            "List the wants pool - aspirational items that could be promoted to needs.",
            json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "description": "Maximum items to return (default: 20)"
                    }
                },
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "mind__want_create",
            "Add an aspirational item to the wants pool for later consideration.",
            json!({
                "type": "object",
                "properties": {
                    "want": {
                        "type": "string",
                        "description": "What we want to accomplish eventually"
                    },
                    "context": {
                        "type": "string",
                        "description": "Supporting information or reasoning"
                    },
                    "priority": {
                        "type": "string",
                        "enum": ["low", "normal", "high", "urgent"],
                        "description": "Priority level (default: normal)"
                    }
                },
                "required": ["want"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "mind__want_remove",
            "Remove an item from the wants pool (completed, no longer relevant, or duplicate).",
            json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The want ID to remove"
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "mind__want_promote",
            "Promote a want to an immediate need (removes from wants, creates need).",
            json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The want ID to promote"
                    },
                    "priority": {
                        "type": "string",
                        "enum": ["low", "normal", "high", "urgent"],
                        "description": "Priority for the need (default: use want's priority)"
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        ),
    ]
}
