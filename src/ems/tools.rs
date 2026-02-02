use serde::Deserialize;
use serde_json::{Value, json};

use crate::llm::ToolSpec;

use super::service::{EmsError, EmsHandle};

pub fn ems_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::function(
            "ems_query",
            "Execute a read-only SQL query against the entity store. Returns rows as JSON objects.",
            json!({
                "type": "object",
                "properties": {
                    "sql": {
                        "type": "string",
                        "description": "SQL SELECT query to execute"
                    },
                    "params": {
                        "type": "array",
                        "items": {},
                        "description": "Bound parameters for the query (optional)"
                    }
                },
                "required": ["sql"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "ems_insert",
            "Insert a row into a table. Auto-creates table and columns as needed. Returns the inserted row.",
            json!({
                "type": "object",
                "properties": {
                    "table": {
                        "type": "string",
                        "description": "Table name (created if not exists)"
                    },
                    "values": {
                        "type": "object",
                        "description": "Column-value pairs to insert. 'id' is auto-generated if not provided."
                    }
                },
                "required": ["table", "values"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "ems_select",
            "Select rows from a table with optional filtering, ordering, and pagination.",
            json!({
                "type": "object",
                "properties": {
                    "table": {
                        "type": "string",
                        "description": "Table name"
                    },
                    "where": {
                        "type": "object",
                        "description": "Filter conditions. Bare values match equality; use $gt, $gte, $lt, $lte, $in for operators."
                    },
                    "columns": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Columns to return (default: all)"
                    },
                    "order_by": {
                        "description": "Sort order. String like 'created_at DESC' or array of strings."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Maximum rows to return"
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Rows to skip"
                    }
                },
                "required": ["table"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "ems_update",
            "Update rows matching a where clause. Returns the number of affected rows.",
            json!({
                "type": "object",
                "properties": {
                    "table": {
                        "type": "string",
                        "description": "Table name"
                    },
                    "where": {
                        "type": "object",
                        "description": "Filter conditions (required). Use bare values for equality or $gt/$lt/etc."
                    },
                    "changes": {
                        "type": "object",
                        "description": "Column-value pairs to update"
                    }
                },
                "required": ["table", "where", "changes"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "ems_delete",
            "Delete rows by their IDs. Returns the number of deleted rows.",
            json!({
                "type": "object",
                "properties": {
                    "table": {
                        "type": "string",
                        "description": "Table name"
                    },
                    "ids": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Row IDs to delete"
                    }
                },
                "required": ["table", "ids"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "ems_describe",
            "Describe tables or a specific table's schema.",
            json!({
                "type": "object",
                "properties": {
                    "table": {
                        "type": "string",
                        "description": "Table name to describe (omit to list all tables)"
                    }
                },
                "additionalProperties": false
            }),
        ),
    ]
}

#[derive(Debug, Deserialize)]
struct QueryArgs {
    sql: String,
    #[serde(default)]
    params: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct InsertArgs {
    table: String,
    values: Value,
}

#[derive(Debug, Deserialize)]
struct SelectArgs {
    table: String,
    #[serde(rename = "where")]
    where_clause: Option<Value>,
    columns: Option<Vec<String>>,
    order_by: Option<Value>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct UpdateArgs {
    table: String,
    #[serde(rename = "where")]
    where_clause: Value,
    changes: Value,
}

#[derive(Debug, Deserialize)]
struct DeleteArgs {
    table: String,
    ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DescribeArgs {
    table: Option<String>,
}

fn ems_ok(data: Value) -> String {
    json!({"ok": true, "data": data}).to_string()
}

fn ems_err(e: EmsError) -> String {
    json!({
        "ok": false,
        "error": {
            "code": e.code,
            "message": e.message
        }
    })
    .to_string()
}

fn parse_err(msg: impl Into<String>) -> String {
    ems_err(EmsError::db(msg))
}

pub async fn exec_ems_tool(ems: &EmsHandle, name: &str, args_json: &str) -> String {
    match name {
        "ems_query" => {
            let args: QueryArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return parse_err(format!("invalid args: {}", e)),
            };

            let guard = match ems.lock() {
                Ok(g) => g,
                Err(e) => return parse_err(format!("lock error: {}", e)),
            };
            match guard.query(&args.sql, &args.params) {
                Ok(rows) => ems_ok(json!({"rows": rows})),
                Err(e) => ems_err(e),
            }
        }

        "ems_insert" => {
            let args: InsertArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return parse_err(format!("invalid args: {}", e)),
            };

            let mut guard = match ems.lock() {
                Ok(g) => g,
                Err(e) => return parse_err(format!("lock error: {}", e)),
            };
            match guard.insert(&args.table, &args.values) {
                Ok(row) => ems_ok(row),
                Err(e) => ems_err(e),
            }
        }

        "ems_select" => {
            let args: SelectArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return parse_err(format!("invalid args: {}", e)),
            };

            let guard = match ems.lock() {
                Ok(g) => g,
                Err(e) => return parse_err(format!("lock error: {}", e)),
            };
            match guard.select(
                &args.table,
                args.where_clause.as_ref(),
                args.columns.as_deref(),
                args.order_by.as_ref(),
                args.limit,
                args.offset,
            ) {
                Ok(rows) => ems_ok(json!({"rows": rows})),
                Err(e) => ems_err(e),
            }
        }

        "ems_update" => {
            let args: UpdateArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return parse_err(format!("invalid args: {}", e)),
            };

            let mut guard = match ems.lock() {
                Ok(g) => g,
                Err(e) => return parse_err(format!("lock error: {}", e)),
            };
            match guard.update(&args.table, &args.where_clause, &args.changes) {
                Ok(n) => ems_ok(json!({"changes": n})),
                Err(e) => ems_err(e),
            }
        }

        "ems_delete" => {
            let args: DeleteArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return parse_err(format!("invalid args: {}", e)),
            };

            let mut guard = match ems.lock() {
                Ok(g) => g,
                Err(e) => return parse_err(format!("lock error: {}", e)),
            };
            match guard.delete(&args.table, &args.ids) {
                Ok(n) => ems_ok(json!({"changes": n})),
                Err(e) => ems_err(e),
            }
        }

        "ems_describe" => {
            let args: DescribeArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return parse_err(format!("invalid args: {}", e)),
            };

            let guard = match ems.lock() {
                Ok(g) => g,
                Err(e) => return parse_err(format!("lock error: {}", e)),
            };
            match guard.describe(args.table.as_deref()) {
                Ok(info) => ems_ok(info),
                Err(e) => ems_err(e),
            }
        }

        _ => parse_err(format!("unknown ems tool: {}", name)),
    }
}
