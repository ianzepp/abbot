use crate::bus::{NeedPriority, Origin, Scope, respond};
use crate::history::Store;
use crate::llm::ToolSpec;
use crate::memory::Search;
use crate::runtime::RuntimeBus;

use globset::{Glob, GlobSet, GlobSetBuilder};

/// Generate a human-readable description of tools from their specs.
/// Format: `- tool_name(param1, param2?, ...) - description`
pub fn describe_tools(specs: &[ToolSpec]) -> String {
    let mut out = String::new();
    out.push_str("## Tools\n\n");

    for spec in specs {
        let name = &spec.function.name;
        let desc = spec.function.description.as_deref().unwrap_or("");
        let params = &spec.function.parameters;

        let mut param_strs = Vec::new();
        if let Some(props) = params.get("properties").and_then(|p| p.as_object()) {
            let required: Vec<&str> = params
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            for (param_name, _param_def) in props {
                if required.contains(&param_name.as_str()) {
                    param_strs.push(param_name.clone());
                } else {
                    param_strs.push(format!("{}?", param_name));
                }
            }
        }

        out.push_str(&format!("- `{}({})` - {}\n", name, param_strs.join(", "), desc));
    }

    out
}
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::fs;
use tokio::process::Command;
use uuid::Uuid;

pub type SharedCwd = Arc<Mutex<PathBuf>>;

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn resolve_from_cwd(&self, cwd: &Path, path: &str) -> Result<PathBuf, ToolError> {
        if path.trim().is_empty() {
            return Err(ToolError::invalid_args("path is empty"));
        }

        let expanded = if path.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(&path[2..])
            } else {
                return Err(ToolError::invalid_args("cannot expand ~: home directory unknown"));
            }
        } else if path == "~" {
            dirs::home_dir().ok_or_else(|| {
                ToolError::invalid_args("cannot expand ~: home directory unknown")
            })?
        } else {
            PathBuf::from(path)
        };

        let joined = if expanded.is_absolute() {
            if !expanded.starts_with(&self.root) {
                return Err(ToolError::outside_workspace(format!(
                    "path {} is outside workspace {}",
                    expanded.display(),
                    self.root.display()
                )));
            }
            expanded
        } else {
            cwd.join(&expanded)
        };

        let normalized = normalize_no_symlinks(&joined);

        if !normalized.starts_with(&self.root) {
            return Err(ToolError::outside_workspace("path escapes workspace"));
        }

        Ok(normalized)
    }
}

fn normalize_no_symlinks(p: &Path) -> PathBuf {
    // Pure lexical normalization: remove "." and fold ".." without touching the filesystem.
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

impl ToolError {
    pub fn invalid_args(msg: impl Into<String>) -> Self {
        Self {
            code: "E_INVALID_ARGS".to_string(),
            message: msg.into(),
            detail: None,
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: "E_NOT_FOUND".to_string(),
            message: msg.into(),
            detail: None,
        }
    }

    pub fn io(msg: impl Into<String>) -> Self {
        Self {
            code: "E_IO".to_string(),
            message: msg.into(),
            detail: None,
        }
    }

    pub fn outside_workspace(msg: impl Into<String>) -> Self {
        Self {
            code: "E_OUTSIDE_WORKSPACE".to_string(),
            message: msg.into(),
            detail: None,
        }
    }

    pub fn patch_failed(msg: impl Into<String>) -> Self {
        Self {
            code: "E_PATCH_FAILED".to_string(),
            message: msg.into(),
            detail: None,
        }
    }
}

pub fn ok(data: Value) -> String {
    json!({"ok": true, "data": data}).to_string()
}

pub fn err(e: ToolError) -> String {
    json!({"ok": false, "error": e}).to_string()
}

pub fn head_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::function(
            "create_task",
            "Queue a task for a hand to execute.",
            json!({
                "type": "object",
                "properties": {
                    "goal": {"type": "string"},
                    "input": {"type": "string"},
                    "notify_scope": {"type": "string"}
                },
                "required": ["goal"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "send_message",
            "Send a chat message to a scope.",
            json!({
                "type": "object",
                "properties": {
                    "scope": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["scope", "content"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "recall",
            "Search indexed transcripts / semantic memory.",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 20}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "introspect",
            "Query system state: messages, wants, logs, stats, needs, goals.",
            json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["messages", "wants", "logs", "stats", "needs", "tasks", "goals"]
                    },
                    "scope": {"type": "string"},
                    "task_id": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100}
                },
                "required": ["mode"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "convene_conclave",
            "Request an immediate conclave for strategic guidance. Use when facing decisions that need Mind-level deliberation.",
            json!({
                "type": "object",
                "properties": {
                    "reason": {"type": "string", "description": "Why you need strategic guidance"}
                },
                "required": ["reason"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "read_file",
            "Read a bounded section of a file. Both offset and limit are required to prevent accidental large reads.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Workspace-relative path to file"},
                    "offset": {"type": "integer", "minimum": 0, "description": "Starting line (0-based)"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "description": "Number of lines to read (max 100)"}
                },
                "required": ["path", "offset", "limit"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "list_files",
            "List files in a directory. max_results is required to prevent unbounded listings.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory path (workspace-relative, defaults to root)"},
                    "pattern": {"type": "string", "description": "Glob pattern to filter files (e.g. *.rs)"},
                    "recursive": {"type": "boolean", "description": "Search subdirectories (default: false)"},
                    "max_results": {"type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum files to return (required, max 50)"}
                },
                "required": ["max_results"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "search_files_goal",
            "Create a goal to search for text in files. Returns task_id; results arrive via task completion.",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Text or pattern to search for"},
                    "path": {"type": "string", "description": "Directory to search in (workspace-relative)"},
                    "include": {"type": "string", "description": "Glob pattern to filter files (e.g. *.rs)"},
                    "regex": {"type": "boolean", "description": "Treat query as regex"},
                    "case_sensitive": {"type": "boolean", "description": "Case-sensitive search"},
                    "max_results": {"type": "integer", "minimum": 1, "maximum": 500, "description": "Maximum matches to return"}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "read_stm",
            "Read the head's short-term memory (STM). Returns current working context.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "update_stm",
            "Update the head's short-term memory (STM). Use to track working context across tasks.",
            json!({
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": ["set", "append", "clear"],
                        "description": "Operation: set (replace), append (add to end), clear (empty)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to set or append (ignored for clear)"
                    }
                },
                "required": ["op"],
                "additionalProperties": false
            }),
        ),
    ]
}

pub fn mind_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::function(
            "update_ltm",
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
            "create_need",
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
            "list_wants",
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
            "add_want",
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
            "remove_want",
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
            "promote_want",
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

pub fn hand_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::function(
            "list_files",
            "List files under a directory (workspace-relative paths).",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "pattern": {"type": "string"},
                    "recursive": {"type": "boolean"},
                    "max_results": {"type": "integer", "minimum": 1, "maximum": 5000}
                },
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "search_files",
            "Search for text in files.",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "path": {"type": "string"},
                    "include": {"type": "string"},
                    "regex": {"type": "boolean"},
                    "case_sensitive": {"type": "boolean"},
                    "max_results": {"type": "integer", "minimum": 1, "maximum": 5000}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "read_file",
            "Read a file (with optional offset/limit by lines).",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer", "minimum": 0},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 2000}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "write_file",
            "Write a file.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                    "create_dirs": {"type": "boolean"},
                    "overwrite": {"type": "boolean"}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "apply_patch",
            "Apply a unified diff patch.",
            json!({
                "type": "object",
                "properties": {
                    "patch": {"type": "string"}
                },
                "required": ["patch"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "diff_files",
            "Compute a unified diff between two files.",
            json!({
                "type": "object",
                "properties": {
                    "a": {"type": "string"},
                    "b": {"type": "string"},
                    "context_lines": {"type": "integer", "minimum": 0, "maximum": 50}
                },
                "required": ["a", "b"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "mkdir",
            "Create a directory.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "parents": {"type": "boolean"}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "echo",
            "Echo text.",
            json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string"}
                },
                "required": ["text"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "add_want",
            "Add an aspirational item to the wants pool. Use when you discover something valuable to do later.",
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
            "git",
            "Run a git command.",
            json!({
                "type": "object",
                "properties": {
                    "args": {
                        "type": "string",
                        "description": "Arguments to pass to git (e.g. \"status\", \"log -n 5\", \"add src/*.rs\")"
                    }
                },
                "required": ["args"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "curl",
            "Make an HTTP request.",
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to request"
                    },
                    "method": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "DELETE"],
                        "description": "HTTP method (default: GET)"
                    },
                    "headers": {
                        "type": "object",
                        "additionalProperties": {"type": "string"},
                        "description": "Additional headers as key-value pairs"
                    },
                    "authorization": {
                        "type": "string",
                        "description": "Authorization header value (convenience for headers.Authorization)"
                    },
                    "content_type": {
                        "type": "string",
                        "description": "Content-Type header value (convenience for headers.Content-Type)"
                    },
                    "body": {
                        "type": "string",
                        "description": "Request body (for POST/PUT/DELETE)"
                    },
                    "timeout": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 30,
                        "description": "Timeout in seconds (default: 30, max: 30)"
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        ),
    ]
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskArgs {
    pub goal: String,
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub notify_scope: String,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageArgs {
    pub scope: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct RecallArgs {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLtmArgs {
    pub ops: Vec<LtmOp>,
}

#[derive(Debug, Deserialize)]
pub struct LtmOp {
    pub kind: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub pattern: String,
}

/// Head-layer read_file: stricter than Hand version (required offset/limit, max 100 lines)
#[derive(Debug, Deserialize)]
pub struct HeadReadFileArgs {
    pub path: String,
    pub offset: usize,
    pub limit: usize,
}

/// Head-layer list_files: stricter than Hand version (required max_results, max 50)
#[derive(Debug, Deserialize)]
pub struct HeadListFilesArgs {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub recursive: bool,
    pub max_results: usize,
}

/// Head-layer search_files_goal: wraps search_files into a goal for Hand execution
#[derive(Debug, Deserialize, Serialize)]
pub struct SearchFilesGoalArgs {
    pub query: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub include: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub regex: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub case_sensitive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ListFilesArgs {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub max_results: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct SearchFilesArgs {
    pub query: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub include: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub max_results: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ReadFileArgs {
    pub path: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub create_dirs: Option<bool>,
    #[serde(default)]
    pub overwrite: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ApplyPatchArgs {
    pub patch: String,
}

#[derive(Debug, Deserialize)]
pub struct DiffFilesArgs {
    pub a: String,
    pub b: String,
    #[serde(default)]
    pub context_lines: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct MkdirArgs {
    pub path: String,
    #[serde(default)]
    pub parents: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct EchoArgs {
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct GitArgs {
    pub args: String,
}

#[derive(Debug, Deserialize)]
pub struct CurlArgs {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub authorization: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
}

pub async fn exec_head_tool(
    bus: &RuntimeBus,
    store: &Store,
    workspace: Option<&Workspace>,
    cwd: Option<&SharedCwd>,
    head_id: &str,
    default_notify_scope: &str,
    reply_to: Option<Uuid>,
    memory: Option<&Arc<Search>>,
    name: &str,
    args_json: &str,
) -> String {
    match name {
        "create_task" => {
            let args: CreateTaskArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };
            let goal = args.goal.trim().to_string();
            if goal.is_empty() {
                return err(ToolError::invalid_args("goal is empty"));
            }

            let task_id = Uuid::new_v4().to_string();
            let scope = Scope::task(&task_id);
            bus.create_scope(scope.clone()).await;

            let notify_scope = if args.notify_scope.trim().is_empty() {
                default_notify_scope.to_string()
            } else {
                args.notify_scope.trim().to_string()
            };

            let mut req = respond::task_request_with_notify(
                head_id,
                scope.clone(),
                &task_id,
                head_id,
                &goal,
                if args.input.trim().is_empty() { &goal } else { args.input.trim() },
                &notify_scope,
            )
            .with_origin(Origin::Head);

            if let Some(r) = reply_to {
                req = req.with_reply_to(r);
            }

            bus.publish(req).await;

            ok(json!({"task_id": task_id, "scope": scope.to_string(), "notify_scope": notify_scope}))
        }
        "send_message" => {
            let args: SendMessageArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };
            if args.scope.trim().is_empty() {
                return err(ToolError::invalid_args("scope is empty"));
            }
            bus.publish(
                respond::chat(head_id, Scope::from(args.scope.as_str()), args.content)
                    .with_origin(Origin::Head),
            )
            .await;
            ok(json!({"sent": true}))
        }
        "recall" => {
            let args: RecallArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };
            let query = args.query.trim();
            if query.is_empty() {
                return err(ToolError::invalid_args("query is empty"));
            }

            let Some(mem) = memory else {
                return err(ToolError {
                    code: "E_MEMORY_DISABLED".to_string(),
                    message: "memory search is not available".to_string(),
                    detail: None,
                });
            };

            let limit = args.limit.unwrap_or(5).clamp(1, 20);
            match mem.query(query, limit).await {
                Ok(results) => {
                    let out = results
                        .into_iter()
                        .map(|r| {
                            json!({
                                "distance": r.distance,
                                "source": r.source,
                                "file_path": r.file_path,
                                "project_path": r.project_path,
                                "content": clip_chars(&r.content, 1200)
                            })
                        })
                        .collect::<Vec<_>>();
                    ok(json!({"results": out}))
                }
                Err(e) => err(ToolError::io(format!("recall error: {e}"))),
            }
        }
        "introspect" => {
            #[derive(Deserialize)]
            struct IntrospectArgs {
                mode: String,
                #[serde(default)]
                scope: Option<String>,
                #[serde(default)]
                task_id: Option<String>,
                #[serde(default)]
                limit: Option<usize>,
            }

            let args: IntrospectArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let limit = args.limit.unwrap_or(20).clamp(1, 100);
            let scope = args.scope.as_deref().unwrap_or("#general");

            match args.mode.as_str() {
                "messages" => {
                    match store.recent(scope, limit) {
                        Ok(msgs) => {
                            let out: Vec<_> = msgs.iter().map(|m| {
                                json!({
                                    "sender": m.sender,
                                    "scope": m.scope.to_string(),
                                    "op": format!("{:?}", m.op),
                                    "data": format!("{:?}", m.data).chars().take(200).collect::<String>()
                                })
                            }).collect();
                            ok(json!({"messages": out, "count": out.len()}))
                        }
                        Err(e) => err(ToolError::io(format!("query error: {e}"))),
                    }
                }
                "wants" => {
                    match store.list_wants(limit) {
                        Ok(wants) => {
                            let out: Vec<_> = wants.iter().map(|w| {
                                json!({
                                    "id": &w.id[..8.min(w.id.len())],
                                    "want": w.want,
                                    "priority": w.priority,
                                    "source": w.source
                                })
                            }).collect();
                            ok(json!({"wants": out, "count": out.len()}))
                        }
                        Err(e) => err(ToolError::io(format!("query error: {e}"))),
                    }
                }
                "logs" => {
                    let Some(task_id) = args.task_id.as_deref() else {
                        return err(ToolError::invalid_args("task_id required for logs mode"));
                    };
                    match store.get_hand_execs(task_id) {
                        Ok(execs) => {
                            let out: Vec<_> = execs.iter().map(|e| {
                                json!({
                                    "step": e.step,
                                    "tool": e.tool,
                                    "success": e.success,
                                    "output": clip_chars(&e.output, 200)
                                })
                            }).collect();
                            ok(json!({"logs": out, "count": out.len()}))
                        }
                        Err(e) => err(ToolError::io(format!("query error: {e}"))),
                    }
                }
                "stats" => {
                    let wants_count = store.count_wants().unwrap_or(0);
                    let recent = store.recent_any(100).unwrap_or_default();
                    let chat_count = recent.iter().filter(|m| matches!(m.op, crate::bus::MessageOp::Chat)).count();
                    let task_count = recent.iter().filter(|m| matches!(m.op, crate::bus::MessageOp::Task)).count();
                    let need_count = recent.iter().filter(|m| matches!(m.op, crate::bus::MessageOp::Need)).count();
                    let error_count = recent.iter().filter(|m| matches!(m.op, crate::bus::MessageOp::Error)).count();

                    ok(json!({
                        "wants_pool": wants_count,
                        "recent_100": {
                            "chat": chat_count,
                            "task": task_count,
                            "need": need_count,
                            "error": error_count
                        }
                    }))
                }
                "needs" => {
                    match store.recent_by_op(scope, "Need", limit) {
                        Ok(msgs) => {
                            let out: Vec<_> = msgs.iter().map(|m| {
                                json!({
                                    "sender": m.sender,
                                    "data": format!("{:?}", m.data).chars().take(200).collect::<String>()
                                })
                            }).collect();
                            ok(json!({"needs": out, "count": out.len()}))
                        }
                        Err(e) => err(ToolError::io(format!("query error: {e}"))),
                    }
                }
                "tasks" | "goals" => {
                    match store.recent_by_op(scope, "Task", limit) {
                        Ok(msgs) => {
                            let out: Vec<_> = msgs.iter().map(|m| {
                                json!({
                                    "sender": m.sender,
                                    "data": format!("{:?}", m.data).chars().take(200).collect::<String>()
                                })
                            }).collect();
                            ok(json!({"tasks": out, "count": out.len()}))
                        }
                        Err(e) => err(ToolError::io(format!("query error: {e}"))),
                    }
                }
                _ => err(ToolError::invalid_args(format!("unknown introspect mode: {}", args.mode))),
            }
        }
        "read_file" => {
            let args: HeadReadFileArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            const MAX_HEAD_READ_LIMIT: usize = 100;
            if args.limit > MAX_HEAD_READ_LIMIT {
                return err(ToolError::invalid_args(format!(
                    "limit {} exceeds maximum {} for head-level read_file",
                    args.limit, MAX_HEAD_READ_LIMIT
                )));
            }

            let (Some(ws), Some(cwd_ref)) = (workspace, cwd) else {
                return err(ToolError::invalid_args("read_file requires workspace context"));
            };

            let cwd_path = cwd_ref.lock().unwrap().clone();
            let full = match ws.resolve_from_cwd(&cwd_path, &args.path) {
                Ok(p) => p,
                Err(e) => return err(e),
            };

            let content = match fs::read_to_string(&full).await {
                Ok(c) => c,
                Err(e) => return err(ToolError::io(format!("read error: {e}"))),
            };

            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();

            if args.offset > total {
                return err(ToolError::invalid_args(format!(
                    "offset {} exceeds file length {}",
                    args.offset, total
                )));
            }

            let end = (args.offset + args.limit).min(total);
            let slice = &lines[args.offset..end];
            let mut out = slice.join("\n");
            if !out.is_empty() {
                out.push('\n');
            }

            ok(json!({
                "path": to_rel(ws.root(), &full),
                "offset": args.offset,
                "limit": args.limit,
                "total_lines": total,
                "truncated": end < total,
                "content": out
            }))
        }
        "list_files" => {
            let args: HeadListFilesArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            const MAX_HEAD_LIST_RESULTS: usize = 50;
            if args.max_results > MAX_HEAD_LIST_RESULTS {
                return err(ToolError::invalid_args(format!(
                    "max_results {} exceeds maximum {} for head-level list_files",
                    args.max_results, MAX_HEAD_LIST_RESULTS
                )));
            }

            let (Some(ws), Some(cwd_ref)) = (workspace, cwd) else {
                return err(ToolError::invalid_args("list_files requires workspace context"));
            };

            let cwd_path = cwd_ref.lock().unwrap().clone();
            let base = if args.path.trim().is_empty() {
                cwd_path
            } else {
                match ws.resolve_from_cwd(&cwd_path, args.path.trim()) {
                    Ok(p) => p,
                    Err(e) => return err(e),
                }
            };

            if !base.exists() {
                return err(ToolError::not_found(format!(
                    "directory not found: {}",
                    args.path
                )));
            }

            let matcher: Option<GlobSet> = if !args.pattern.trim().is_empty() {
                let glob = Glob::new(args.pattern.trim())
                    .map_err(|e| ToolError::invalid_args(format!("invalid pattern: {e}")));
                let glob = match glob {
                    Ok(g) => g,
                    Err(e) => return err(e),
                };
                let mut builder = GlobSetBuilder::new();
                builder.add(glob);
                builder.build().ok()
            } else {
                None
            };

            let mut out = Vec::new();
            let depth = if args.recursive { usize::MAX } else { 1 };
            for entry in walkdir::WalkDir::new(&base)
                .follow_links(false)
                .max_depth(depth)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.path() == base {
                    continue;
                }
                let rel = entry
                    .path()
                    .strip_prefix(ws.root())
                    .ok()
                    .map(|p| p.to_string_lossy().to_string());
                let Some(rel) = rel else { continue };

                if let Some(m) = &matcher {
                    let name = entry.file_name().to_string_lossy();
                    if !m.is_match(name.as_ref()) {
                        continue;
                    }
                }

                out.push(rel);
                if out.len() >= args.max_results {
                    break;
                }
            }

            out.sort();
            ok(json!({"matches": out, "truncated": out.len() >= args.max_results}))
        }
        "search_files_goal" => {
            let args: SearchFilesGoalArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            if args.query.trim().is_empty() {
                return err(ToolError::invalid_args("query is empty"));
            }

            let task_id = Uuid::new_v4().to_string();
            let scope = Scope::task(&task_id);
            bus.create_scope(scope.clone()).await;

            let notify_scope = default_notify_scope.to_string();

            // Serialize args as structured input for the Hand
            let input_json = serde_json::to_string(&args).unwrap_or_default();

            let mut req = respond::task_request_with_notify(
                head_id,
                scope.clone(),
                &task_id,
                head_id,
                "search_files",
                &input_json,
                &notify_scope,
            )
            .with_origin(Origin::Head);

            if let Some(r) = reply_to {
                req = req.with_reply_to(r);
            }

            bus.publish(req).await;

            ok(json!({"task_id": task_id, "scope": scope.to_string(), "notify_scope": notify_scope}))
        }
        "convene_conclave" => {
            #[derive(Deserialize)]
            struct ConveneConclaveArgs {
                reason: String,
            }

            let args: ConveneConclaveArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            if args.reason.trim().is_empty() {
                return err(ToolError::invalid_args("reason is empty"));
            }

            let msg = respond::event(
                head_id,
                Scope::from("@mind"),
                "convene_conclave",
                json!({"reason": args.reason, "requested_by": head_id}),
            )
            .with_origin(Origin::Head);

            bus.publish(msg).await;

            ok(json!({"requested": true, "reason": args.reason}))
        }
        "read_stm" => {
            let stm = store.get_head_stm(head_id).unwrap_or_default();
            ok(json!({
                "head_id": head_id,
                "stm": stm,
                "len": stm.len()
            }))
        }
        "update_stm" => {
            #[derive(Deserialize)]
            struct UpdateStmArgs {
                op: String,
                #[serde(default)]
                content: String,
            }

            let args: UpdateStmArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let current = store.get_head_stm(head_id).unwrap_or_default();
            let new_stm = match args.op.as_str() {
                "set" => args.content.clone(),
                "append" => {
                    if current.is_empty() {
                        args.content.clone()
                    } else if args.content.is_empty() {
                        current
                    } else {
                        format!("{}\n\n{}", current, args.content)
                    }
                }
                "clear" => String::new(),
                _ => return err(ToolError::invalid_args(format!("unknown op: {}", args.op))),
            };

            if let Err(e) = store.set_head_stm(head_id, &new_stm) {
                return err(ToolError::io(format!("failed to save STM: {e}")));
            }

            ok(json!({
                "head_id": head_id,
                "op": args.op,
                "stm_len": new_stm.len()
            }))
        }
        _ => err(ToolError::invalid_args(format!("unknown tool: {name}"))),
    }
}

pub async fn exec_mind_tool(
    bus: &RuntimeBus,
    store: &Store,
    head_id: &str,
    name: &str,
    args_json: &str,
) -> String {
    match name {
        "create_need" => {
            #[derive(Deserialize)]
            struct CreateNeedArgs {
                need: String,
                #[serde(default)]
                context: String,
                #[serde(default)]
                priority: Option<String>,
            }

            let args: CreateNeedArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let priority = match args.priority.as_deref() {
                Some("low") => NeedPriority::Low,
                Some("high") => NeedPriority::High,
                Some("urgent") => NeedPriority::Urgent,
                _ => NeedPriority::Normal,
            };

            let need_id = Uuid::new_v4().to_string();

            let msg = respond::need_request(
                "mind",
                Scope::from("@need_service"),
                &need_id,
                "mind",
                priority,
                &args.need,
                &args.context,
            )
            .with_origin(Origin::System);

            bus.publish(msg).await;

            ok(json!({
                "need_id": need_id,
                "priority": format!("{:?}", priority),
                "status": "queued"
            }))
        }
        "update_ltm" => {
            let args: UpdateLtmArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let current = store.get_head_ltm(head_id).unwrap_or_default();
            let mut ltm = current.clone();
            let mut applied = Vec::new();

            for op in args.ops {
                match op.kind.as_str() {
                    "append" => {
                        let content = op.content.trim();
                        if content.is_empty() {
                            continue;
                        }
                        if !ltm.is_empty() {
                            ltm.push_str("\n\n");
                        }
                        ltm.push_str(content);
                        applied.push(json!({"kind":"append"}));
                    }
                    "replace" => {
                        let pattern = op.pattern;
                        if pattern.is_empty() {
                            continue;
                        }
                        if let Some(pos) = ltm.find(&pattern) {
                            let end = pos + pattern.len();
                            let replacement = op.content;
                            ltm.replace_range(pos..end, &replacement);
                            applied.push(json!({"kind":"replace", "pattern": pattern}));
                        }
                    }
                    "remove" => {
                        let pattern = op.pattern;
                        if pattern.is_empty() {
                            continue;
                        }
                        if ltm.contains(&pattern) {
                            ltm = ltm.replace(&pattern, "");
                            while ltm.contains("\n\n\n") {
                                ltm = ltm.replace("\n\n\n", "\n\n");
                            }
                            ltm = ltm.trim().to_string();
                            applied.push(json!({"kind":"remove", "pattern": pattern}));
                        }
                    }
                    _ => {}
                }
            }

            if ltm != current {
                if let Err(e) = store.set_head_ltm(head_id, &ltm) {
                    return err(ToolError::io(format!("failed to save LTM: {e}")));
                }
            }

            ok(json!({"applied": applied, "ltm_len": ltm.len()}))
        }
        "list_wants" => {
            #[derive(Deserialize)]
            struct ListWantsArgs {
                #[serde(default)]
                limit: Option<usize>,
            }

            let args: ListWantsArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let limit = args.limit.unwrap_or(20).clamp(1, 100);
            match store.list_wants(limit) {
                Ok(wants) => {
                    let items: Vec<_> = wants
                        .into_iter()
                        .map(|w| {
                            json!({
                                "id": w.id,
                                "want": w.want,
                                "context": w.context,
                                "priority": w.priority,
                                "source": w.source
                            })
                        })
                        .collect();
                    ok(json!({"wants": items, "count": items.len()}))
                }
                Err(e) => err(ToolError::io(format!("failed to list wants: {e}"))),
            }
        }
        "add_want" => {
            #[derive(Deserialize)]
            struct AddWantArgs {
                want: String,
                #[serde(default)]
                context: String,
                #[serde(default)]
                priority: Option<String>,
            }

            let args: AddWantArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let priority = args.priority.as_deref().unwrap_or("normal");
            let want_id = Uuid::new_v4().to_string();

            match store.add_want(&want_id, &args.want, &args.context, priority, "mind") {
                Ok(()) => ok(json!({
                    "want_id": want_id,
                    "priority": priority,
                    "status": "added"
                })),
                Err(e) => err(ToolError::io(format!("failed to add want: {e}"))),
            }
        }
        "remove_want" => {
            #[derive(Deserialize)]
            struct RemoveWantArgs {
                id: String,
            }

            let args: RemoveWantArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            match store.remove_want(&args.id) {
                Ok(true) => ok(json!({"removed": true})),
                Ok(false) => ok(json!({"removed": false, "reason": "not found"})),
                Err(e) => err(ToolError::io(format!("failed to remove want: {e}"))),
            }
        }
        "promote_want" => {
            #[derive(Deserialize)]
            struct PromoteWantArgs {
                id: String,
                #[serde(default)]
                priority: Option<String>,
            }

            let args: PromoteWantArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            // Get the want first
            let want = match store.get_want(&args.id) {
                Ok(Some(w)) => w,
                Ok(None) => return ok(json!({"promoted": false, "reason": "want not found"})),
                Err(e) => return err(ToolError::io(format!("failed to get want: {e}"))),
            };

            // Remove the want
            if let Err(e) = store.remove_want(&args.id) {
                return err(ToolError::io(format!("failed to remove want: {e}")));
            }

            // Create the need with priority override if provided
            let priority_str = args.priority.as_deref().unwrap_or(&want.priority);
            let priority = match priority_str {
                "low" => NeedPriority::Low,
                "high" => NeedPriority::High,
                "urgent" => NeedPriority::Urgent,
                _ => NeedPriority::Normal,
            };

            let need_id = Uuid::new_v4().to_string();

            let msg = respond::need_request(
                "mind",
                Scope::from("@need_service"),
                &need_id,
                "mind",
                priority,
                &want.want,
                &want.context,
            )
            .with_origin(Origin::System);

            bus.publish(msg).await;

            ok(json!({
                "promoted": true,
                "want_id": args.id,
                "need_id": need_id,
                "priority": format!("{:?}", priority)
            }))
        }
        _ => err(ToolError::invalid_args(format!("unknown tool: {name}"))),
    }
}

pub async fn exec_hand_tool(
    workspace: &Workspace,
    cwd: &SharedCwd,
    store: &Store,
    name: &str,
    args_json: &str,
) -> String {
    match name {
        "list_files" => {
            let args: ListFilesArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };
            let max_results = args.max_results.unwrap_or(1000).min(5000);

            let cwd_path = cwd.lock().unwrap().clone();
            let base = if args.path.trim().is_empty() {
                cwd_path
            } else {
                match workspace.resolve_from_cwd(&cwd_path, args.path.trim()) {
                    Ok(p) => p,
                    Err(e) => return err(e),
                }
            };

            if !base.exists() {
                return err(ToolError::not_found(format!(
                    "directory not found: {}",
                    args.path
                )));
            }

            let matcher: Option<GlobSet> = if !args.pattern.trim().is_empty() {
                let glob = Glob::new(args.pattern.trim())
                    .map_err(|e| ToolError::invalid_args(format!("invalid pattern: {e}")));
                let glob = match glob {
                    Ok(g) => g,
                    Err(e) => return err(e),
                };
                let mut builder = GlobSetBuilder::new();
                builder.add(glob);
                builder.build().ok()
            } else {
                None
            };

            let mut out = Vec::new();
            let depth = if args.recursive { usize::MAX } else { 1 };
            for entry in walkdir::WalkDir::new(&base)
                .follow_links(false)
                .max_depth(depth)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.path() == base {
                    continue;
                }
                let rel = entry
                    .path()
                    .strip_prefix(workspace.root())
                    .ok()
                    .map(|p| p.to_string_lossy().to_string());
                let Some(rel) = rel else { continue };

                if let Some(m) = &matcher {
                    let name = entry.file_name().to_string_lossy();
                    if !m.is_match(name.as_ref()) {
                        continue;
                    }
                }

                out.push(rel);
                if out.len() >= max_results {
                    break;
                }
            }

            out.sort();
            ok(json!({"matches": out, "truncated": out.len() >= max_results}))
        }

        "search_files" => {
            let args: SearchFilesArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };
            let query = args.query;
            if query.trim().is_empty() {
                return err(ToolError::invalid_args("query is empty"));
            }
            let max_results = args.max_results.unwrap_or(200).min(5000);

            let cwd_path = cwd.lock().unwrap().clone();
            let base = if args.path.trim().is_empty() {
                cwd_path
            } else {
                match workspace.resolve_from_cwd(&cwd_path, args.path.trim()) {
                    Ok(p) => p,
                    Err(e) => return err(e),
                }
            };

            if !base.exists() {
                return err(ToolError::not_found(format!(
                    "directory not found: {}",
                    args.path
                )));
            }

            let include = args.include.trim().to_string();
            let include_set = if include.is_empty() {
                None
            } else {
                let mut b = GlobSetBuilder::new();
                let g = match Glob::new(&include)
                    .map_err(|e| ToolError::invalid_args(format!("invalid include: {e}")))
                {
                    Ok(v) => v,
                    Err(e) => return err(e),
                };
                b.add(g);
                let set = match b
                    .build()
                    .map_err(|e| ToolError::invalid_args(format!("invalid include: {e}")))
                {
                    Ok(v) => v,
                    Err(e) => return err(e),
                };
                Some(set)
            };

            let re = if args.regex {
                let pat = if args.case_sensitive { query.clone() } else { format!("(?i){}", query) };
                match Regex::new(&pat).map_err(|e| ToolError::invalid_args(format!("invalid regex: {e}"))) {
                    Ok(r) => Some(r),
                    Err(e) => return err(e),
                }
            } else {
                None
            };

            let mut matches = Vec::new();
            for entry in walkdir::WalkDir::new(&base)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let file_name = entry.file_name().to_string_lossy();
                if let Some(inc) = &include_set {
                    if !inc.is_match(file_name.as_ref()) {
                        continue;
                    }
                }

                let path = entry.path();
                let content = match fs::read_to_string(path).await {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                for (i, line) in content.lines().enumerate() {
                    let hit = if let Some(re) = &re {
                        re.is_match(line)
                    } else if args.case_sensitive {
                        line.contains(&query)
                    } else {
                        line.to_ascii_lowercase().contains(&query.to_ascii_lowercase())
                    };

                    if hit {
                        let rel = path
                            .strip_prefix(workspace.root())
                            .ok()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.to_string_lossy().to_string());
                        matches.push(json!({
                            "path": rel,
                            "line": i + 1,
                            "text": clip_chars(line, 400)
                        }));
                        if matches.len() >= max_results {
                            break;
                        }
                    }
                }
                if matches.len() >= max_results {
                    break;
                }
            }

            ok(json!({"matches": matches, "truncated": matches.len() >= max_results}))
        }

        "read_file" => {
            let args: ReadFileArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };
            let cwd_path = cwd.lock().unwrap().clone();
            let full = match workspace.resolve_from_cwd(&cwd_path, &args.path) {
                Ok(p) => p,
                Err(e) => return err(e),
            };
            let content = match fs::read_to_string(&full).await {
                Ok(c) => c,
                Err(e) => return err(ToolError::io(format!("read error: {e}"))),
            };

            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();
            let offset = args.offset.unwrap_or(0);
            let limit = args.limit.unwrap_or(200).min(2000);
            if offset > total {
                return err(ToolError::invalid_args(format!(
                    "offset {} exceeds file length {}",
                    offset, total
                )));
            }
            let end = (offset + limit).min(total);
            let slice = &lines[offset..end];
            let mut out = slice.join("\n");
            if !out.is_empty() {
                out.push('\n');
            }
            let truncated = end < total;
            ok(json!({
                "path": to_rel(workspace.root(), &full),
                "offset": offset,
                "limit": limit,
                "total_lines": total,
                "truncated": truncated,
                "content": out
            }))
        }

        "write_file" => {
            let args: WriteFileArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };
            let create_dirs = args.create_dirs.unwrap_or(false);
            let overwrite = args.overwrite.unwrap_or(true);
            let cwd_path = cwd.lock().unwrap().clone();
            let full = match workspace.resolve_from_cwd(&cwd_path, &args.path) {
                Ok(p) => p,
                Err(e) => return err(e),
            };

            if create_dirs {
                if let Some(parent) = full.parent() {
                    if let Err(e) = fs::create_dir_all(parent).await {
                        return err(ToolError::io(format!("mkdir error: {e}")));
                    }
                }
            }

            if !overwrite {
                if fs::try_exists(&full).await.unwrap_or(false) {
                    return err(ToolError {
                        code: "E_EXISTS".to_string(),
                        message: "file exists and overwrite=false".to_string(),
                        detail: Some(json!({"path": to_rel(workspace.root(), &full)})),
                    });
                }
            }

            if let Err(e) = fs::write(&full, &args.content).await {
                return err(ToolError::io(format!("write error: {e}")));
            }

            ok(json!({"path": to_rel(workspace.root(), &full), "bytes": args.content.len()}))
        }

        "apply_patch" => {
            let args: ApplyPatchArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };
            let diff = args.patch.trim();
            if diff.is_empty() {
                return err(ToolError::invalid_args("patch is empty"));
            }
            if !diff.contains("---") || !diff.contains("+++") {
                return err(ToolError::invalid_args(
                    "invalid diff format (missing --- or +++ headers)",
                ));
            }

            let cwd_path = cwd.lock().unwrap().clone();

            // Validate all paths in diff headers are within workspace
            for line in diff.lines() {
                let path = if let Some(rest) = line.strip_prefix("+++ ") {
                    Some(rest)
                } else if let Some(rest) = line.strip_prefix("--- ") {
                    Some(rest)
                } else {
                    None
                };

                if let Some(raw_path) = path {
                    // Skip /dev/null (used for new/deleted files)
                    if raw_path == "/dev/null" || raw_path.starts_with("/dev/null") {
                        continue;
                    }

                    // Strip "a/" or "b/" prefix (git diff style), then any trailing tab+timestamp
                    let stripped = raw_path
                        .strip_prefix("a/")
                        .or_else(|| raw_path.strip_prefix("b/"))
                        .unwrap_or(raw_path)
                        .split('\t')
                        .next()
                        .unwrap_or(raw_path);

                    if stripped.is_empty() {
                        continue;
                    }

                    if let Err(e) = workspace.resolve_from_cwd(&cwd_path, stripped) {
                        return err(ToolError::outside_workspace(format!(
                            "patch references path outside workspace: {} ({})",
                            stripped, e.message
                        )));
                    }
                }
            }

            let mut child = match Command::new("patch")
                .arg("-p1")
                .arg("--no-backup-if-mismatch")
                .arg("-r-")
                .current_dir(&cwd_path)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => return err(ToolError::io(format!("spawn patch: {e}"))),
            };

            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                if let Err(e) = stdin.write_all(diff.as_bytes()).await {
                    return err(ToolError::io(format!("write patch stdin: {e}")));
                }
            }

            match child.wait_with_output().await {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    if output.status.success() {
                        ok(json!({"stdout": clip_chars(stdout.trim_end(), 4000)}))
                    } else {
                        let msg = if !stderr.trim().is_empty() {
                            stderr.trim().to_string()
                        } else {
                            stdout.trim().to_string()
                        };
                        err(ToolError::patch_failed(msg))
                    }
                }
                Err(e) => err(ToolError::io(format!("patch error: {e}"))),
            }
        }

        "diff_files" => {
            let args: DiffFilesArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };
            let cwd_path = cwd.lock().unwrap().clone();
            let a = match workspace.resolve_from_cwd(&cwd_path, &args.a) {
                Ok(p) => p,
                Err(e) => return err(e),
            };
            let b = match workspace.resolve_from_cwd(&cwd_path, &args.b) {
                Ok(p) => p,
                Err(e) => return err(e),
            };

            let context = args.context_lines.unwrap_or(3);
            let out = Command::new("diff")
                .arg("-U")
                .arg(context.to_string())
                .arg(&a)
                .arg(&b)
                .output()
                .await;

            match out {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                    if stdout.trim().is_empty() {
                        if output.status.success() {
                            ok(json!({"diff": ""}))
                        } else {
                            err(ToolError::io(stderr.trim().to_string()))
                        }
                    } else {
                        ok(json!({"diff": clip_chars(&stdout, 20_000)}))
                    }
                }
                Err(e) => err(ToolError::io(format!("diff error: {e}"))),
            }
        }

        "mkdir" => {
            let args: MkdirArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };
            let parents = args.parents.unwrap_or(true);
            let cwd_path = cwd.lock().unwrap().clone();
            let full = match workspace.resolve_from_cwd(&cwd_path, &args.path) {
                Ok(p) => p,
                Err(e) => return err(e),
            };

            let exists = fs::try_exists(&full).await.unwrap_or(false);
            if exists {
                return ok(json!({"created": false, "path": to_rel(workspace.root(), &full)}));
            }

            let res = if parents {
                fs::create_dir_all(&full).await
            } else {
                fs::create_dir(&full).await
            };

            match res {
                Ok(_) => ok(json!({"created": true, "path": to_rel(workspace.root(), &full)})),
                Err(e) => err(ToolError::io(format!("mkdir error: {e}"))),
            }
        }

        "echo" => {
            let args: EchoArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };
            ok(json!({"text": args.text}))
        }

        "git" => {
            let args: GitArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let args_str = args.args.trim();
            if args_str.is_empty() {
                return err(ToolError::invalid_args("args is empty"));
            }

            let cwd_path = cwd.lock().unwrap().clone();

            let output = Command::new("git")
                .args(args_str.split_whitespace())
                .current_dir(&cwd_path)
                .output()
                .await;

            match output {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let code = output.status.code().unwrap_or(-1);

                    if output.status.success() {
                        ok(json!({
                            "stdout": clip_chars(stdout.trim(), 50_000),
                            "stderr": clip_chars(stderr.trim(), 5_000),
                            "code": code
                        }))
                    } else {
                        err(ToolError {
                            code: "E_GIT".to_string(),
                            message: if !stderr.trim().is_empty() {
                                clip_chars(stderr.trim(), 2000)
                            } else {
                                clip_chars(stdout.trim(), 2000)
                            },
                            detail: Some(json!({"code": code})),
                        })
                    }
                }
                Err(e) => err(ToolError::io(format!("git error: {e}"))),
            }
        }

        "add_want" => {
            #[derive(Deserialize)]
            struct AddWantArgs {
                want: String,
                #[serde(default)]
                context: String,
                #[serde(default)]
                priority: Option<String>,
            }

            let args: AddWantArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let priority = args.priority.as_deref().unwrap_or("normal");
            let want_id = Uuid::new_v4().to_string();

            match store.add_want(&want_id, &args.want, &args.context, priority, "hand") {
                Ok(()) => ok(json!({
                    "want_id": want_id,
                    "priority": priority,
                    "status": "added"
                })),
                Err(e) => err(ToolError::io(format!("failed to add want: {e}"))),
            }
        }

        "curl" => {
            let args: CurlArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let url = args.url.trim();
            if url.is_empty() {
                return err(ToolError::invalid_args("url is empty"));
            }

            let method = args.method.as_deref().unwrap_or("GET").to_uppercase();
            let timeout_secs = args.timeout.unwrap_or(30).min(30);

            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .build()
            {
                Ok(c) => c,
                Err(e) => return err(ToolError::io(format!("failed to create HTTP client: {e}"))),
            };

            let mut request = match method.as_str() {
                "GET" => client.get(url),
                "POST" => client.post(url),
                "PUT" => client.put(url),
                "DELETE" => client.delete(url),
                _ => return err(ToolError::invalid_args(format!("unsupported method: {method}"))),
            };

            if let Some(auth) = &args.authorization {
                request = request.header("Authorization", auth);
            }
            if let Some(ct) = &args.content_type {
                request = request.header("Content-Type", ct);
            }
            if let Some(hdrs) = &args.headers {
                for (k, v) in hdrs {
                    request = request.header(k, v);
                }
            }
            if let Some(body) = &args.body {
                request = request.body(body.clone());
            }

            match request.send().await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let headers: std::collections::HashMap<String, String> = response
                        .headers()
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect();

                    const MAX_BODY_SIZE: usize = 256 * 1024;
                    match response.bytes().await {
                        Ok(bytes) => {
                            let truncated = bytes.len() > MAX_BODY_SIZE;
                            let body_bytes = if truncated {
                                &bytes[..MAX_BODY_SIZE]
                            } else {
                                &bytes[..]
                            };

                            let body_str = String::from_utf8_lossy(body_bytes).to_string();

                            ok(json!({
                                "status": status,
                                "headers": headers,
                                "body": body_str,
                                "truncated": truncated,
                                "body_size": bytes.len()
                            }))
                        }
                        Err(e) => err(ToolError::io(format!("failed to read response body: {e}"))),
                    }
                }
                Err(e) => {
                    if e.is_timeout() {
                        err(ToolError {
                            code: "E_TIMEOUT".to_string(),
                            message: format!("request timed out after {}s", timeout_secs),
                            detail: None,
                        })
                    } else if e.is_connect() {
                        err(ToolError {
                            code: "E_CONNECT".to_string(),
                            message: format!("connection failed: {e}"),
                            detail: None,
                        })
                    } else {
                        err(ToolError::io(format!("request failed: {e}")))
                    }
                }
            }
        }

        _ => err(ToolError::invalid_args(format!("unknown tool: {name}"))),
    }
}

fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

fn to_rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .ok()
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|| p.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_test_workspace() -> (TempDir, Workspace, SharedCwd) {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        
        // Create test files
        File::create(root.join("file1.txt")).unwrap().write_all(b"content1").unwrap();
        File::create(root.join("file2.py")).unwrap().write_all(b"print('hello')").unwrap();
        File::create(root.join("file3.rs")).unwrap().write_all(b"fn main() {}").unwrap();
        fs::create_dir(root.join("subdir")).unwrap();
        File::create(root.join("subdir/nested.txt")).unwrap().write_all(b"nested").unwrap();
        
        let workspace = Workspace::new(root.clone());
        let cwd: SharedCwd = Arc::new(Mutex::new(root));
        
        (temp, workspace, cwd)
    }

    #[test]
    fn test_list_files_no_pattern() {
        let (_temp, workspace, cwd) = setup_test_workspace();
        
        let args = HeadListFilesArgs {
            path: String::new(),
            pattern: String::new(),
            recursive: false,
            max_results: 50,
        };
        
        let mut builder = GlobSetBuilder::new();
        let matcher: Option<GlobSet> = if !args.pattern.trim().is_empty() {
            builder.add(Glob::new(args.pattern.trim()).unwrap());
            builder.build().ok()
        } else {
            None
        };
        
        let cwd_path = cwd.lock().unwrap().clone();
        let base = cwd_path;
        
        let mut out = Vec::new();
        let depth = if args.recursive { usize::MAX } else { 1 };
        for entry in walkdir::WalkDir::new(&base)
            .follow_links(false)
            .max_depth(depth)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.path() == base {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(workspace.root())
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            let Some(rel) = rel else { continue };

            if let Some(m) = &matcher {
                let name = entry.file_name().to_string_lossy();
                if !m.is_match(name.as_ref()) {
                    continue;
                }
            }

            out.push(rel);
            if out.len() >= args.max_results {
                break;
            }
        }
        
        out.sort();
        assert_eq!(out.len(), 4); // file1.txt, file2.py, file3.rs, subdir
        assert!(out.contains(&"file1.txt".to_string()));
        assert!(out.contains(&"file2.py".to_string()));
        assert!(out.contains(&"file3.rs".to_string()));
        assert!(out.contains(&"subdir".to_string()));
    }

    #[test]
    fn test_list_files_with_pattern() {
        let (_temp, workspace, cwd) = setup_test_workspace();
        
        let args = HeadListFilesArgs {
            path: String::new(),
            pattern: "*.py".to_string(),
            recursive: false,
            max_results: 50,
        };
        
        let matcher: Option<GlobSet> = if !args.pattern.trim().is_empty() {
            let mut builder = GlobSetBuilder::new();
            builder.add(Glob::new(args.pattern.trim()).unwrap());
            builder.build().ok()
        } else {
            None
        };
        
        let cwd_path = cwd.lock().unwrap().clone();
        let base = cwd_path;
        
        let mut out = Vec::new();
        let depth = if args.recursive { usize::MAX } else { 1 };
        for entry in walkdir::WalkDir::new(&base)
            .follow_links(false)
            .max_depth(depth)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.path() == base {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(workspace.root())
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            let Some(rel) = rel else { continue };

            if let Some(m) = &matcher {
                let name = entry.file_name().to_string_lossy();
                if !m.is_match(name.as_ref()) {
                    continue;
                }
            }

            out.push(rel);
            if out.len() >= args.max_results {
                break;
            }
        }
        
        out.sort();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], "file2.py");
    }

    #[test]
    fn test_list_files_recursive() {
        let (_temp, workspace, cwd) = setup_test_workspace();
        
        let args = HeadListFilesArgs {
            path: String::new(),
            pattern: "*.txt".to_string(),
            recursive: true,
            max_results: 50,
        };
        
        let matcher: Option<GlobSet> = if !args.pattern.trim().is_empty() {
            let mut builder = GlobSetBuilder::new();
            builder.add(Glob::new(args.pattern.trim()).unwrap());
            builder.build().ok()
        } else {
            None
        };
        
        let cwd_path = cwd.lock().unwrap().clone();
        let base = cwd_path;
        
        let mut out = Vec::new();
        let depth = if args.recursive { usize::MAX } else { 1 };
        for entry in walkdir::WalkDir::new(&base)
            .follow_links(false)
            .max_depth(depth)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.path() == base {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(workspace.root())
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            let Some(rel) = rel else { continue };

            if let Some(m) = &matcher {
                let name = entry.file_name().to_string_lossy();
                if !m.is_match(name.as_ref()) {
                    continue;
                }
            }

            out.push(rel);
            if out.len() >= args.max_results {
                break;
            }
        }
        
        out.sort();
        assert_eq!(out.len(), 2); // file1.txt, subdir/nested.txt
        assert!(out.contains(&"file1.txt".to_string()));
        assert!(out.iter().any(|s| s.ends_with("nested.txt")));
    }

    #[test]
    fn test_list_files_max_results() {
        let (_temp, workspace, cwd) = setup_test_workspace();
        
        let args = HeadListFilesArgs {
            path: String::new(),
            pattern: String::new(),
            recursive: false,
            max_results: 2,
        };
        
        let matcher: Option<GlobSet> = None;
        
        let cwd_path = cwd.lock().unwrap().clone();
        let base = cwd_path;
        
        let mut out = Vec::new();
        let depth = if args.recursive { usize::MAX } else { 1 };
        for entry in walkdir::WalkDir::new(&base)
            .follow_links(false)
            .max_depth(depth)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.path() == base {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(workspace.root())
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            let Some(rel) = rel else { continue };

            if let Some(m) = &matcher {
                let name = entry.file_name().to_string_lossy();
                if !m.is_match(name.as_ref()) {
                    continue;
                }
            }

            out.push(rel);
            if out.len() >= args.max_results {
                break;
            }
        }
        
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_empty_globset_matches_nothing() {
        // This test documents the behavior that caused the bug
        let builder = GlobSetBuilder::new();
        let globset = builder.build().unwrap();
        
        // An empty GlobSet matches nothing
        assert!(!globset.is_match("anything.txt"));
        assert!(!globset.is_match("test.py"));
        assert!(!globset.is_match(""));
    }
}
