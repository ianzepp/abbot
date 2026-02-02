use crate::bus::{NeedPriority, Origin, Scope, respond};
use crate::ems::{EmsHandle, ems_tool_specs, exec_ems_tool};
use crate::history::Store;
use crate::llm::{LlmClient, ToolSpec, UnifiedMessage};
use crate::recall::Search;
use crate::runtime::{RuntimeBus, TaskServiceQuery};

use globset::{Glob, GlobSet, GlobSetBuilder};

/// Tool side-effect classification for access control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEffect {
    /// Tool only reads data, no mutations.
    ReadOnly,
    /// Tool may modify workspace, files, or external state.
    Mutating,
}

/// Classify a hand tool by its side effects.
/// Returns None if tool name is unknown.
pub fn hand_tool_effect(name: &str) -> Option<ToolEffect> {
    let canonical = canonical_hand_tool_name(name);
    match canonical {
        // Read-only tools
        "list_files" | "search_files" | "read_file" | "diff_files" | "echo" => {
            Some(ToolEffect::ReadOnly)
        }
        // Mutating tools
        "write_file" | "apply_patch" | "mkdir" | "git" | "add_want" => Some(ToolEffect::Mutating),
        // HTTP: depends on method, but classify as potentially mutating
        "curl" => Some(ToolEffect::Mutating),
        // LLM calls are read-only (no local mutations)
        "chat_completion" => Some(ToolEffect::ReadOnly),
        // EMS read-only tools
        "ems_query" | "ems_select" | "ems_describe" => Some(ToolEffect::ReadOnly),
        _ => None,
    }
}

/// Check if an HTTP method is read-only.
pub fn is_http_method_readonly(method: &str) -> bool {
    matches!(method.to_uppercase().as_str(), "GET" | "HEAD" | "OPTIONS")
}

/// Read-only git subcommands (safe for hands).
const GIT_READONLY_SUBCOMMANDS: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "branch",
    "tag",
    "remote",
    "ls-files",
    "ls-tree",
    "cat-file",
    "rev-parse",
    "rev-list",
    "describe",
    "shortlog",
    "blame",
    "bisect",
    "stash list",
];

/// Check if git args represent a read-only operation.
pub fn is_git_readonly(args: &str) -> bool {
    let first_arg = args.split_whitespace().next().unwrap_or("");
    GIT_READONLY_SUBCOMMANDS.contains(&first_arg)
}

/// Classify a head tool by its side effects.
/// Returns None if tool name is unknown.
pub fn head_tool_effect(name: &str) -> Option<ToolEffect> {
    let canonical = canonical_head_tool_name(name);
    match canonical {
        // Read-only tools
        "recall" | "introspect" | "explain_tool" | "read_file" | "list_files"
        | "search_files_goal" | "read_stm" | "read_config" | "list_models" | "chat_completion"
        | "list_tasks" | "read_task" | "search_tasks" => Some(ToolEffect::ReadOnly),
        // Mutating tools
        "create_task" | "send_message" | "convene_conclave" | "consult" | "update_stm"
        | "update_config" => Some(ToolEffect::Mutating),
        // Workspace mutation tools (added for heads)
        "write_file" | "apply_patch" | "mkdir" | "git" | "curl" => Some(ToolEffect::Mutating),
        // EMS tools
        "ems_query" | "ems_select" | "ems_describe" => Some(ToolEffect::ReadOnly),
        "ems_insert" | "ems_update" | "ems_delete" => Some(ToolEffect::Mutating),
        _ => None,
    }
}

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

        out.push_str(&format!(
            "- `{}({})` - {}\n",
            name,
            param_strs.join(", "),
            desc
        ));
    }

    out
}
use crate::hal::{
    HalFs, HalGit, HalHttpRequest, HalNet, HalProcess, HostHalFs, HostHalGit, HostHalNet,
    HostHalProcess,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::fs;
use tokio_util::sync::CancellationToken;
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
                return Err(ToolError::invalid_args(
                    "cannot expand ~: home directory unknown",
                ));
            }
        } else if path == "~" {
            dirs::home_dir()
                .ok_or_else(|| ToolError::invalid_args("cannot expand ~: home directory unknown"))?
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

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self {
            code: "E_FORBIDDEN".to_string(),
            message: msg.into(),
            detail: None,
        }
    }

    pub fn db(msg: impl Into<String>) -> Self {
        Self {
            code: "E_DB".to_string(),
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

fn canonical_head_tool_name(name: &str) -> &str {
    match name {
        // Canonicalize to implementation (legacy) names.
        // Tool specs advertise the new names; dispatch accepts both.
        "tasks_create" => "create_task",
        "tasks_list" => "list_tasks",
        "tasks_read" => "read_task",
        "tasks_search" => "search_tasks",
        "chat_send" => "send_message",
        "memory_recall" => "recall",
        "state_query" => "introspect",
        "conclave_request" => "convene_conclave",
        "advisor_consult" => "consult",
        "tools_explain" => "explain_tool",
        "fs_read_excerpt" => "read_file",
        "fs_list_brief" => "list_files",
        "goals_create_fs_search" => "search_files_goal",
        "memory_stm_read" => "read_stm",
        "memory_stm_update" => "update_stm",
        "config_read" => "read_config",
        "config_update" => "update_config",
        "models_list" => "list_models",
        "llm_chat" => "chat_completion",

        // Mind tools (executed via exec_head_tool)
        "memory_ltm_update" => "update_ltm",
        "needs_create" => "create_need",
        "wants_list" => "list_wants",
        "wants_create" => "add_want",
        "wants_remove" => "remove_want",
        "wants_promote" => "promote_want",

        // Workspace mutation tools (heads only)
        "fs_write" => "write_file",
        "patch_apply" => "apply_patch",
        "fs_mkdir" => "mkdir",
        "git_run" => "git",
        "http_request" => "curl",

        _ => name,
    }
}

fn canonical_hand_tool_name(name: &str) -> &str {
    match name {
        // Canonicalize to implementation (legacy) names.
        // Tool specs advertise the new names; dispatch accepts both.
        "fs_list" => "list_files",
        "fs_search" => "search_files",
        "fs_read" => "read_file",
        "fs_write" => "write_file",
        "patch_apply" => "apply_patch",
        "fs_diff" => "diff_files",
        "fs_mkdir" => "mkdir",
        "text_echo" => "echo",
        "wants_create" => "add_want",
        "git_run" => "git",
        "http_request" => "curl",
        "http_get" => "http_get",
        "llm_chat" => "chat_completion",
        _ => name,
    }
}

/// Hand tools that are strictly read-only (allowed for hands).
const HAND_READONLY_TOOLS: &[&str] = &[
    "list_files",
    "search_files",
    "read_file",
    "diff_files",
    "echo",
    "http_get",
    "chat_completion",
    "ems_query",
    "ems_select",
    "ems_describe",
];

/// Check if a hand tool (canonical name) is allowed for hands.
pub fn is_hand_tool_allowed(canonical_name: &str) -> bool {
    HAND_READONLY_TOOLS.contains(&canonical_name)
}

pub fn head_tool_specs() -> Vec<ToolSpec> {
    let mut specs = vec![
        ToolSpec::function(
            "tasks_create",
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
            "tasks_list",
            "List tasks by status.",
            json!({
                "type": "object",
                "properties": {
                    "status": {"type": "string", "enum": ["pending", "running", "completed", "all"]},
                    "scope": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100}
                },
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "tasks_read",
            "Read task details and execution logs.",
            json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"}
                },
                "required": ["task_id"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "tasks_search",
            "Search task goals and results.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "scope": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 50}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "chat_send",
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
            "memory_recall",
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
            "state_query",
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
            "conclave_request",
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
            "advisor_consult",
            "Consult HeadManager for tactical advice. Returns structured guidance; optionally emits a note into chat.",
            json!({
                "type": "object",
                "properties": {
                    "case": {"type": "string", "description": "Current situation / goal"},
                    "attempted": {"type": "string", "description": "What you tried / current plan"},
                    "ask": {"type": "string", "description": "What you want from HeadManager"},
                    "visibility": {"type": "string", "enum": ["silent", "note", "chat"], "description": "Whether to publish a note with the consult result"},
                    "max_tokens": {"type": "integer", "minimum": 200, "maximum": 2000, "description": "Max tokens for the consult response"}
                },
                "required": ["case"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "tools_explain",
            "Explain a tool by name. Use to fetch full details/schema for external tools.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "scope": {"type": "string", "description": "Optional scope override; defaults to the current conversation scope"},
                    "source": {"type": "string", "enum": ["external"], "description": "Tool source (currently only external)"}
                },
                "required": ["name"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "fs_read_excerpt",
            "Read a bounded section of a file. If the result is truncated, this tool returns an error (E_TRUNCATED) and you must delegate or page.",
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
            "fs_list_brief",
            "List files in a directory. If the result is truncated, this tool returns an error (E_TRUNCATED) and you must delegate or narrow the query.",
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
            "goals_create_fs_search",
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
            "memory_stm_read",
            "Read the head's short-term memory (STM). Returns current working context.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "memory_stm_update",
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
        ToolSpec::function(
            "config_read",
            "Read workspace config. Returns entire config, a section, or a specific key.",
            json!({
                "type": "object",
                "properties": {
                    "section": {
                        "type": "string",
                        "description": "Config section (e.g., 'head', 'dials')"
                    },
                    "key": {
                        "type": "string",
                        "description": "Key within section"
                    }
                },
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "config_update",
            "Update a workspace config value. Use to change dials, model, or other settings.",
            json!({
                "type": "object",
                "properties": {
                    "section": {
                        "type": "string",
                        "description": "Config section (e.g., 'head', 'hand', 'mind', 'tars', 'harness')"
                    },
                    "key": {
                        "type": "string",
                        "description": "Key within section"
                    },
                    "value": {
                        "description": "Value to set (string, number, or boolean)"
                    }
                },
                "required": ["section", "key", "value"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "models_list",
            "List available models that can be used for head/hand/mind.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "llm_chat",
            "Make a one-shot LLM request to any configured model.",
            json!({
                "type": "object",
                "properties": {
                    "model": {
                        "type": "string",
                        "description": "Model ID from models.toml (e.g., 'anthropic/claude-sonnet-4-20250514')"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The user prompt to send"
                    },
                    "system": {
                        "type": "string",
                        "description": "Optional system prompt"
                    },
                    "temperature": {
                        "type": "number",
                        "minimum": 0.0,
                        "maximum": 2.0,
                        "description": "Sampling temperature (default: model default)"
                    },
                    "max_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 16000,
                        "description": "Maximum tokens in response (default: 4096)"
                    }
                },
                "required": ["model", "prompt"],
                "additionalProperties": false
            }),
        ),
        // Workspace mutation tools (heads can directly modify the workspace)
        ToolSpec::function(
            "fs_write",
            "Write a file directly (heads only).",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Workspace-relative path"},
                    "content": {"type": "string", "description": "File content to write"},
                    "create_dirs": {"type": "boolean", "description": "Create parent directories if missing"},
                    "overwrite": {"type": "boolean", "description": "Overwrite existing file (default: true)"}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "patch_apply",
            "Apply a unified diff patch directly (heads only).",
            json!({
                "type": "object",
                "properties": {
                    "patch": {"type": "string", "description": "Unified diff patch to apply"}
                },
                "required": ["patch"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "fs_mkdir",
            "Create a directory (heads only).",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Workspace-relative path"},
                    "parents": {"type": "boolean", "description": "Create parent directories (default: true)"}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "git_run",
            "Run a git command (heads only).",
            json!({
                "type": "object",
                "properties": {
                    "args": {
                        "type": "string",
                        "description": "Arguments to pass to git (e.g. \"status\", \"add src/*.rs\", \"commit -m 'msg'\")"
                    }
                },
                "required": ["args"],
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "http_request",
            "Make an HTTP request (heads only, supports all methods).",
            json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "The URL to request"},
                    "method": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "DELETE"],
                        "description": "HTTP method (default: GET)"
                    },
                    "headers": {
                        "type": "object",
                        "additionalProperties": {"type": "string"},
                        "description": "Additional headers"
                    },
                    "authorization": {"type": "string", "description": "Authorization header value"},
                    "content_type": {"type": "string", "description": "Content-Type header value"},
                    "body": {"type": "string", "description": "Request body (for POST/PUT/DELETE)"},
                    "timeout": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 30,
                        "description": "Timeout in seconds (default: 30)"
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        ),
    ];
    specs.extend(ems_tool_specs());
    specs
}

pub fn mind_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::function(
            "memory_ltm_update",
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
            "needs_create",
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
            "wants_list",
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
            "wants_create",
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
            "wants_remove",
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
            "wants_promote",
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
    let mut specs = vec![
        ToolSpec::function(
            "fs_list",
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
            "fs_search",
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
            "fs_read",
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
            "fs_diff",
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
            "text_echo",
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
            "http_get",
            "Make a read-only HTTP GET request.",
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to request"
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
        ToolSpec::function(
            "llm_chat",
            "Make a one-shot LLM request to any configured model.",
            json!({
                "type": "object",
                "properties": {
                    "model": {
                        "type": "string",
                        "description": "Model ID from models.toml (e.g., 'anthropic/claude-sonnet-4-20250514')"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The user prompt to send"
                    },
                    "system": {
                        "type": "string",
                        "description": "Optional system prompt"
                    },
                    "temperature": {
                        "type": "number",
                        "minimum": 0.0,
                        "maximum": 2.0,
                        "description": "Sampling temperature (default: model default)"
                    },
                    "max_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 16000,
                        "description": "Maximum tokens in response (default: 4096)"
                    }
                },
                "required": ["model", "prompt"],
                "additionalProperties": false
            }),
        ),
    ];
    // Add read-only EMS tools for hands
    let ems_readonly = ["ems_query", "ems_select", "ems_describe"];
    specs.extend(
        ems_tool_specs()
            .into_iter()
            .filter(|t| ems_readonly.contains(&t.function.name.as_str())),
    );
    specs
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
pub struct TasksListArgs {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct TasksReadArgs {
    pub task_id: String,
}

#[derive(Debug, Deserialize)]
pub struct TasksSearchArgs {
    pub pattern: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
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
pub struct ChatCompletionArgs {
    pub model: String,
    pub prompt: String,
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
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
    task_query: Option<&TaskServiceQuery>,
    ems: Option<&EmsHandle>,
    scope: &str,
    name: &str,
    args_json: &str,
) -> String {
    let _ = scope; // Reserved for future kernel syscall routing
    let name = canonical_head_tool_name(name);

    let workspace_root = || {
        workspace
            .map(|w| w.root().to_path_buf())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    };

    if name.starts_with("ems_") {
        return match ems {
            Some(h) => exec_ems_tool(h, name, args_json).await,
            None => err(ToolError::db("EMS service not available")),
        };
    }

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

            let notify_scope = if args.notify_scope.trim().is_empty() {
                default_notify_scope.to_string()
            } else {
                args.notify_scope.trim().to_string()
            };

            if let Some(k) = crate::runtime::Kernel::get() {
                let dispatcher = k.dispatcher().await;
                let req = crate::kernel::Frame::req(
                    "task:enqueue",
                    json!({
                        "task_id": task_id,
                        "head_id": head_id,
                        "goal": goal,
                        "input": args.input,
                        "scope": notify_scope,
                        "notify_scope": notify_scope,
                        "reply_to": reply_to.map(|u| u.to_string()),
                    }),
                )
                .with_actor(format!("head/{head_id}"));

                let mut rx = dispatcher.dispatch(
                    req,
                    workspace_root(),
                    tokio_util::sync::CancellationToken::new(),
                );
                let _ = rx.recv().await;
            }

            ok(json!({"task_id": task_id, "notify_scope": notify_scope}))
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
                "messages" => match store.recent(scope, limit) {
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
                },
                "wants" => match store.list_wants(limit) {
                    Ok(wants) => {
                        let out: Vec<_> = wants
                            .iter()
                            .map(|w| {
                                json!({
                                    "id": &w.id[..8.min(w.id.len())],
                                    "want": w.want,
                                    "priority": w.priority,
                                    "source": w.source
                                })
                            })
                            .collect();
                        ok(json!({"wants": out, "count": out.len()}))
                    }
                    Err(e) => err(ToolError::io(format!("query error: {e}"))),
                },
                "logs" => {
                    let Some(task_id) = args.task_id.as_deref() else {
                        return err(ToolError::invalid_args("task_id required for logs mode"));
                    };
                    match store.get_hand_execs(task_id) {
                        Ok(execs) => {
                            let out: Vec<_> = execs
                                .iter()
                                .map(|e| {
                                    json!({
                                        "step": e.step,
                                        "tool": e.tool,
                                        "success": e.success,
                                        "output": clip_chars(&e.output, 200)
                                    })
                                })
                                .collect();
                            ok(json!({"logs": out, "count": out.len()}))
                        }
                        Err(e) => err(ToolError::io(format!("query error: {e}"))),
                    }
                }
                "stats" => {
                    let wants_count = store.count_wants().unwrap_or(0);
                    let recent = store.recent_any(100).unwrap_or_default();
                    let chat_count = recent
                        .iter()
                        .filter(|m| matches!(m.op, crate::bus::MessageOp::Chat))
                        .count();
                    let task_count = recent
                        .iter()
                        .filter(|m| matches!(m.op, crate::bus::MessageOp::Task))
                        .count();
                    let need_count = recent
                        .iter()
                        .filter(|m| matches!(m.op, crate::bus::MessageOp::Need))
                        .count();
                    let error_count = recent
                        .iter()
                        .filter(|m| matches!(m.op, crate::bus::MessageOp::Error))
                        .count();

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
                "needs" => match store.recent_by_op(scope, "Need", limit) {
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
                },
                "tasks" | "goals" => match store.recent_by_op(scope, "Task", limit) {
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
                },
                _ => err(ToolError::invalid_args(format!(
                    "unknown introspect mode: {}",
                    args.mode
                ))),
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
                return err(ToolError::invalid_args(
                    "read_file requires workspace context",
                ));
            };

            let cwd_path = cwd_ref.lock().unwrap().clone();
            let full = match ws.resolve_from_cwd(&cwd_path, &args.path) {
                Ok(p) => p,
                Err(e) => return err(e),
            };

            let content = match HostHalFs::default().read_to_string(&full).await {
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

            let truncated = end < total;
            let data = json!({
                "path": to_rel(ws.root(), &full),
                "offset": args.offset,
                "limit": args.limit,
                "total_lines": total,
                "truncated": truncated,
                "content": out
            });

            if truncated {
                return err(ToolError {
                    code: "E_TRUNCATED".to_string(),
                    message: "read_file returned truncated output; delegate to a hand for full context or continue paging with a higher offset".to_string(),
                    detail: Some(data),
                });
            }

            ok(data)
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
                return err(ToolError::invalid_args(
                    "list_files requires workspace context",
                ));
            };

            let cwd_path = cwd_ref.lock().unwrap().clone();
            let base = if args.path.trim().is_empty() {
                cwd_path
            } else {
                if std::path::Path::new(args.path.trim()).is_absolute() {
                    return err(ToolError::invalid_args(
                        "list_files path must be workspace-relative; use an external client tool for absolute user paths",
                    ));
                }
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
            let truncated = out.len() >= args.max_results;
            let data =
                json!({"matches": out, "truncated": truncated, "max_results": args.max_results});

            if truncated {
                return err(ToolError {
                    code: "E_TRUNCATED".to_string(),
                    message: "list_files returned truncated output; delegate to a hand for complete results or narrow the query".to_string(),
                    detail: Some(data),
                });
            }

            ok(data)
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

            ok(
                json!({"task_id": task_id, "scope": scope.to_string(), "notify_scope": notify_scope}),
            )
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
        "consult" => {
            use crate::llm::{ChatMessage, OpenAICompatClient, Role};
            use crate::runtime::HeadConfig;

            #[derive(Deserialize)]
            struct ConsultArgs {
                #[serde(rename = "case")]
                case_text: String,
                #[serde(default)]
                attempted: String,
                #[serde(default)]
                ask: String,
                #[serde(default)]
                visibility: String,
                #[serde(default)]
                max_tokens: Option<u32>,
            }

            fn extract_jsonish(s: &str) -> String {
                let blocks = crate::runtime::parser::parse_fenced_blocks(s);
                if let Some(b) = blocks
                    .iter()
                    .find(|b| b.tag.trim().eq_ignore_ascii_case("json"))
                {
                    let trimmed = b.content.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_string();
                    }
                }
                let trimmed = s.trim();
                if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
                    if end > start {
                        return trimmed[start..=end].to_string();
                    }
                }
                trimmed.to_string()
            }

            fn clip(s: &str, max: usize) -> String {
                let s = s.replace('\n', " ");
                if s.chars().count() <= max {
                    return s;
                }
                let clipped: String = s.chars().take(max).collect();
                format!("{}...", clipped)
            }

            let args: ConsultArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let case_text = args.case_text.trim();
            if case_text.is_empty() {
                return err(ToolError::invalid_args("case is empty"));
            }

            let cfg = HeadConfig::from_env();
            if !cfg.llm.enabled {
                return err(ToolError {
                    code: "E_LLM_DISABLED".to_string(),
                    message: "head LLM not configured".to_string(),
                    detail: None,
                });
            }

            let max_tokens = args.max_tokens.or(cfg.llm.max_tokens).or(Some(800));

            let client = OpenAICompatClient::new(
                &cfg.llm.base_url,
                &cfg.llm.api_key,
                &cfg.llm.model,
                cfg.llm.temperature,
                max_tokens,
                cfg.llm.extra_headers.clone(),
            );

            let system = format!(
                "{}\n\n{}",
                include_str!("runtime/head_manager.md").trim(),
                include_str!("runtime/head_consult.md").trim()
            );

            let user_prompt = format!(
                "## Case\n{}\n\n## Attempted\n{}\n\n## Ask\n{}\n",
                case_text,
                args.attempted.trim(),
                args.ask.trim()
            );

            let messages = vec![
                ChatMessage::new(Role::System, system),
                ChatMessage::new(Role::User, user_prompt),
            ];

            let response = match client.chat(messages).await {
                Ok(r) => r,
                Err(e) => {
                    return err(ToolError {
                        code: "E_LLM".to_string(),
                        message: format!("consult failed: {e}"),
                        detail: None,
                    });
                }
            };

            let raw = response.content;
            let json_str = extract_jsonish(&raw);
            let parsed: serde_json::Value = match serde_json::from_str(&json_str) {
                Ok(v) => v,
                Err(e) => {
                    return err(ToolError {
                        code: "E_PARSE".to_string(),
                        message: format!("consult response was not valid JSON: {e}"),
                        detail: Some(json!({"raw": raw})),
                    });
                }
            };

            let visibility = if args.visibility.trim().is_empty() {
                "silent"
            } else {
                args.visibility.trim()
            };

            if matches!(visibility, "note" | "chat") {
                let scope = Scope::from(default_notify_scope);
                let diag = parsed
                    .get("advice")
                    .and_then(|v| v.get("diagnosis"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no diagnosis)");

                let content = if visibility == "note" {
                    format!("HeadManager consult: {}", clip(diag, 240))
                } else {
                    let pretty =
                        serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| raw.clone());
                    format!("HeadManager consult:\n{}", pretty)
                };

                let mut chat = respond::chat(head_id, scope, content).with_origin(Origin::Head);
                if let Some(r) = reply_to {
                    chat = chat.with_reply_to(r);
                }
                bus.publish(chat).await;
            }

            ok(json!({"consult": parsed}))
        }
        "explain_tool" => {
            #[derive(Deserialize)]
            struct ExplainToolArgs {
                name: String,
                #[serde(default)]
                scope: Option<String>,
                #[serde(default)]
                source: Option<String>,
            }

            let args: ExplainToolArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };
            let tool_name = args.name.trim();
            if tool_name.is_empty() {
                return err(ToolError::invalid_args("name is empty"));
            }

            let tool_name = tool_name.strip_prefix("client__").unwrap_or(tool_name);

            let scope = args
                .scope
                .as_deref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or(default_notify_scope);

            let source = args
                .source
                .as_deref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("external");

            if source != "external" {
                return err(ToolError::invalid_args("unsupported source"));
            }

            match store.get_tool(scope, source, tool_name) {
                Ok(Some(t)) => ok(json!({
                    "scope": scope,
                    "source": source,
                    "name": t.name,
                    "summary": t.summary,
                    "description": t.description,
                    "schema_json": t.schema_json,
                })),
                Ok(None) => err(ToolError {
                    code: "E_TOOL_NOT_FOUND".to_string(),
                    message: format!("tool not found: {} (source={})", tool_name, source),
                    detail: Some(json!({"scope": scope})),
                }),
                Err(e) => err(ToolError::io(format!("db error: {e}"))),
            }
        }
        "read_stm" => {
            let path = crate::runtime::workspace_head_memory(&workspace_root(), head_id);
            let stm = match crate::runtime::read_optional_file(&path) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    // One-time migration from legacy DB location.
                    let legacy = store.get_head_stm(head_id).unwrap_or_default();
                    if !legacy.trim().is_empty() {
                        let _ = crate::runtime::atomic_write_file_0600(&path, legacy.trim());
                        legacy
                    } else {
                        String::new()
                    }
                }
                Err(_) => String::new(),
            };
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

            let path = crate::runtime::workspace_head_memory(&workspace_root(), head_id);
            let current = match crate::runtime::read_optional_file(&path) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    // One-time migration from legacy DB location.
                    let legacy = store.get_head_stm(head_id).unwrap_or_default();
                    if !legacy.trim().is_empty() {
                        let _ = crate::runtime::atomic_write_file_0600(&path, legacy.trim());
                        legacy
                    } else {
                        String::new()
                    }
                }
                Err(_) => String::new(),
            };
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

            if let Err(e) = crate::runtime::atomic_write_file_0600(&path, &new_stm) {
                return err(ToolError::io(format!("failed to save STM file: {e}")));
            }

            ok(json!({
                "head_id": head_id,
                "op": args.op,
                "stm_len": new_stm.len()
            }))
        }
        "read_config" => {
            #[derive(Deserialize)]
            struct ReadConfigArgs {
                section: Option<String>,
                key: Option<String>,
            }

            let args: ReadConfigArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let config_path = crate::runtime::workspace_config_from_root(&workspace_root());

            let config_str = match crate::runtime::read_optional_file(&config_path) {
                Ok(Some(s)) => s,
                Ok(None) => String::new(),
                Err(e) => return err(ToolError::io(format!("failed to read config: {e}"))),
            };

            let config: toml::Table = if config_str.is_empty() {
                toml::Table::new()
            } else {
                match config_str.parse() {
                    Ok(t) => t,
                    Err(e) => return err(ToolError::io(format!("invalid config TOML: {e}"))),
                }
            };

            match (args.section.as_deref(), args.key.as_deref()) {
                (None, None) => ok(json!(config)),
                (Some(section), None) => {
                    let value = config
                        .get(section)
                        .cloned()
                        .unwrap_or(toml::Value::Table(toml::Table::new()));
                    ok(json!({ "section": section, "value": value }))
                }
                (Some(section), Some(key)) => {
                    let value = config
                        .get(section)
                        .and_then(|s| s.as_table())
                        .and_then(|t| t.get(key))
                        .cloned();
                    ok(json!({ "section": section, "key": key, "value": value }))
                }
                (None, Some(_)) => err(ToolError::invalid_args("key requires section")),
            }
        }
        "update_config" => {
            #[derive(Deserialize)]
            struct UpdateConfigArgs {
                section: String,
                key: String,
                value: serde_json::Value,
            }

            let args: UpdateConfigArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let config_path = crate::runtime::workspace_config_from_root(&workspace_root());

            let config_str = match crate::runtime::read_optional_file(&config_path) {
                Ok(Some(s)) => s,
                Ok(None) => String::new(),
                Err(e) => return err(ToolError::io(format!("failed to read config: {e}"))),
            };

            let mut config: toml::Table = if config_str.is_empty() {
                toml::Table::new()
            } else {
                match config_str.parse() {
                    Ok(t) => t,
                    Err(e) => return err(ToolError::io(format!("invalid config TOML: {e}"))),
                }
            };

            let section_table = config
                .entry(&args.section)
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut();

            let Some(section_table) = section_table else {
                return err(ToolError::invalid_args(format!(
                    "section '{}' is not a table",
                    args.section
                )));
            };

            let toml_value = json_to_toml(&args.value);
            section_table.insert(args.key.clone(), toml_value);

            let new_config_str = toml::to_string_pretty(&config).unwrap_or_default();
            if let Err(e) = crate::runtime::atomic_write_file_0600(&config_path, &new_config_str) {
                return err(ToolError::io(format!("failed to write config: {e}")));
            }

            ok(json!({
                "section": args.section,
                "key": args.key,
                "value": args.value,
                "status": "updated"
            }))
        }
        "list_models" => {
            let models = crate::runtime::ModelsConfig::global();
            let model_list: Vec<_> = models
                .models
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id,
                        "provider": m.provider,
                        "context_window": m.context_window,
                        "supports_tools": m.supports_tools,
                        "supports_vision": m.supports_vision,
                    })
                })
                .collect();
            ok(json!({ "models": model_list }))
        }

        // Workspace mutation tools (heads only)
        "write_file" => {
            let (workspace, cwd) = match (workspace, cwd) {
                (Some(w), Some(c)) => (w, c),
                _ => return err(ToolError::invalid_args("workspace not available")),
            };
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
                    if let Err(e) = HostHalFs::default().create_dir_all(parent).await {
                        return err(ToolError::io(format!("mkdir error: {e}")));
                    }
                }
            }

            if !overwrite {
                if HostHalFs::default().exists(&full).await.unwrap_or(false) {
                    return err(ToolError {
                        code: "E_EXISTS".to_string(),
                        message: "file exists and overwrite=false".to_string(),
                        detail: Some(json!({"path": to_rel(workspace.root(), &full)})),
                    });
                }
            }

            if let Err(e) = HostHalFs::default()
                .write(&full, args.content.as_bytes())
                .await
            {
                return err(ToolError::io(format!("write error: {e}")));
            }

            ok(json!({"path": to_rel(workspace.root(), &full), "bytes": args.content.len()}))
        }

        "apply_patch" => {
            let (workspace, cwd) = match (workspace, cwd) {
                (Some(w), Some(c)) => (w, c),
                _ => return err(ToolError::invalid_args("workspace not available")),
            };
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
                    if raw_path == "/dev/null" || raw_path.starts_with("/dev/null") {
                        continue;
                    }
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

            let argv = vec![
                "-p1".to_string(),
                "--no-backup-if-mismatch".to_string(),
                "-r-".to_string(),
            ];

            let output = HostHalProcess::default()
                .run_with_stdin_bytes_bounded(
                    "patch",
                    &argv,
                    &cwd_path,
                    None,
                    None,
                    diff.as_bytes(),
                    256 * 1024,
                    256 * 1024,
                    None,
                )
                .await;

            match output {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    if output.success {
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
                Err(crate::hal::process::HalProcessError::Cancelled { .. }) => err(ToolError {
                    code: "E_CANCELLED".to_string(),
                    message: "patch cancelled".to_string(),
                    detail: None,
                }),
                Err(e) => err(ToolError::io(format!("patch error: {e}"))),
            }
        }

        "mkdir" => {
            let (workspace, cwd) = match (workspace, cwd) {
                (Some(w), Some(c)) => (w, c),
                _ => return err(ToolError::invalid_args("workspace not available")),
            };
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

            let exists = HostHalFs::default().exists(&full).await.unwrap_or(false);
            if exists {
                return ok(json!({"created": false, "path": to_rel(workspace.root(), &full)}));
            }

            let res = if parents {
                HostHalFs::default().create_dir_all(&full).await
            } else {
                HostHalFs::default().create_dir(&full).await
            };

            match res {
                Ok(_) => ok(json!({"created": true, "path": to_rel(workspace.root(), &full)})),
                Err(e) => err(ToolError::io(format!("mkdir error: {e}"))),
            }
        }

        "git" => {
            let cwd = match cwd {
                Some(c) => c,
                None => return err(ToolError::invalid_args("workspace not available")),
            };
            let args: GitArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let args_str = args.args.trim();
            if args_str.is_empty() {
                return err(ToolError::invalid_args("args is empty"));
            }

            let cwd_path = cwd.lock().unwrap().clone();

            let argv: Vec<String> = args_str.split_whitespace().map(|s| s.to_string()).collect();
            let output = HostHalGit::default().run(&cwd_path, &argv, None).await;

            match output {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let code = output.code;

                    if output.success {
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

            match method.as_str() {
                "GET" | "POST" | "PUT" | "DELETE" => {}
                _ => {
                    return err(ToolError::invalid_args(format!(
                        "unsupported method: {method}"
                    )));
                }
            };

            let mut headers: std::collections::HashMap<String, String> =
                args.headers.clone().unwrap_or_default();
            if let Some(auth) = &args.authorization {
                headers.insert("Authorization".to_string(), auth.clone());
            }
            if let Some(ct) = &args.content_type {
                headers.insert("Content-Type".to_string(), ct.clone());
            }

            const MAX_BODY_SIZE: usize = 256 * 1024;
            let hal = HostHalNet::default();
            let resp = hal
                .http_request(HalHttpRequest {
                    method: method.clone(),
                    url: url.to_string(),
                    headers,
                    body: args.body.as_ref().map(|b| b.as_bytes().to_vec()),
                    timeout: std::time::Duration::from_secs(timeout_secs),
                    max_body_bytes: MAX_BODY_SIZE,
                })
                .await;

            match resp {
                Ok(resp) => {
                    let body_str = String::from_utf8_lossy(&resp.body).to_string();
                    ok(json!({
                        "status": resp.status,
                        "headers": resp.headers,
                        "body": body_str,
                        "truncated": resp.truncated,
                        "body_size": resp.body_size
                    }))
                }
                Err(crate::hal::net::HalNetError::Timeout { .. }) => err(ToolError {
                    code: "E_TIMEOUT".to_string(),
                    message: format!("request timed out after {}s", timeout_secs),
                    detail: None,
                }),
                Err(crate::hal::net::HalNetError::Connect(msg)) => err(ToolError {
                    code: "E_CONNECT".to_string(),
                    message: format!("connection failed: {msg}"),
                    detail: None,
                }),
                Err(e) => err(ToolError::io(format!("request failed: {e}"))),
            }
        }

        "list_tasks" => {
            let args: TasksListArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let status_filter = args.status.as_deref().unwrap_or("all");
            let limit = args.limit.unwrap_or(50).clamp(1, 100);

            let mut tasks = Vec::new();

            // Get live tasks from TaskServiceQuery if available
            if let Some(tq) = task_query {
                // Get pending tasks
                if matches!(status_filter, "pending" | "all") {
                    for task in tq.pending_tasks().await {
                        if let Some(scope) = &args.scope {
                            if !task.scope.to_string().contains(scope) {
                                continue;
                            }
                        }
                        tasks.push(json!({
                            "id": task.id,
                            "status": "pending",
                            "goal": clip_chars(&task.goal, 200),
                            "head_id": task.head_id,
                            "scope": task.scope.to_string(),
                        }));
                    }
                }

                // Get running tasks
                if matches!(status_filter, "running" | "all") {
                    let hands = tq.hand_status().await;
                    for hand in hands {
                        if let crate::runtime::HandState::Running {
                            task_id,
                            head_id: running_head_id,
                            ..
                        } = hand.state
                        {
                            if let Some(task) = tq.get_task(&task_id).await {
                                if let Some(scope) = &args.scope {
                                    if !task.scope.to_string().contains(scope) {
                                        continue;
                                    }
                                }
                                tasks.push(json!({
                                    "id": task.id,
                                    "status": "running",
                                    "goal": clip_chars(&task.goal, 200),
                                    "head_id": running_head_id,
                                    "hand_id": hand.hand_id,
                                    "scope": task.scope.to_string(),
                                }));
                            }
                        }
                    }
                }
            }

            // Get completed tasks from store
            if matches!(status_filter, "completed" | "all") {
                let scope_str = args.scope.as_deref().unwrap_or("#main");
                if let Ok(msgs) = store.recent_by_op(scope_str, "Task", limit) {
                    for msg in msgs {
                        if let crate::bus::MessageData::Task(crate::bus::TaskMsg::Result {
                            task_id,
                            ok,
                            summary,
                            ..
                        }) = &msg.data
                        {
                            tasks.push(json!({
                                "id": task_id,
                                "status": if *ok { "completed" } else { "failed" },
                                "summary": clip_chars(summary, 200),
                            }));
                        }
                    }
                }
            }

            // Limit results
            tasks.truncate(limit);

            ok(json!({
                "tasks": tasks,
                "count": tasks.len(),
                "status_filter": status_filter
            }))
        }

        "read_task" => {
            let args: TasksReadArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let task_id = args.task_id.trim();
            if task_id.is_empty() {
                return err(ToolError::invalid_args("task_id is empty"));
            }

            // Check live tasks first
            if let Some(tq) = task_query {
                if let Some(task) = tq.get_task(task_id).await {
                    // Determine if running
                    let mut status = "pending";
                    let mut hand_id: Option<String> = None;
                    for hand in tq.hand_status().await {
                        if let crate::runtime::HandState::Running {
                            task_id: running_id,
                            ..
                        } = &hand.state
                        {
                            if running_id == task_id {
                                status = "running";
                                hand_id = Some(hand.hand_id.clone());
                                break;
                            }
                        }
                    }

                    return ok(json!({
                        "id": task.id,
                        "status": status,
                        "goal": task.goal,
                        "head_id": task.head_id,
                        "scope": task.scope.to_string(),
                        "hand_id": hand_id,
                    }));
                }
            }

            // Fall back to store for completed tasks
            match store.get_hand_execs(task_id) {
                Ok(execs) => {
                    if execs.is_empty() {
                        return err(ToolError::not_found(format!("task not found: {}", task_id)));
                    }

                    let logs: Vec<_> = execs
                        .iter()
                        .map(|e| {
                            json!({
                                "step": e.step,
                                "tool": e.tool,
                                "success": e.success,
                                "output": clip_chars(&e.output, 500)
                            })
                        })
                        .collect();

                    ok(json!({
                        "id": task_id,
                        "status": "completed",
                        "execution_log": logs,
                        "steps": logs.len()
                    }))
                }
                Err(e) => err(ToolError::io(format!("query error: {e}"))),
            }
        }

        "search_tasks" => {
            let args: TasksSearchArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let pattern = args.pattern.trim();
            if pattern.is_empty() {
                return err(ToolError::invalid_args("pattern is empty"));
            }

            let limit = args.limit.unwrap_or(20).clamp(1, 50);
            let pattern_lower = pattern.to_lowercase();

            let mut matches = Vec::new();

            // Search live tasks
            if let Some(tq) = task_query {
                for task in tq.pending_tasks().await {
                    if task.goal.to_lowercase().contains(&pattern_lower) {
                        if let Some(scope) = &args.scope {
                            if !task.scope.to_string().contains(scope) {
                                continue;
                            }
                        }
                        matches.push(json!({
                            "id": task.id,
                            "status": "pending",
                            "goal": clip_chars(&task.goal, 200),
                            "match_in": "goal",
                        }));
                    }
                }

                for task in tq.active_tasks().await {
                    if task.goal.to_lowercase().contains(&pattern_lower) {
                        if let Some(scope) = &args.scope {
                            if !task.scope.to_string().contains(scope) {
                                continue;
                            }
                        }
                        matches.push(json!({
                            "id": task.id,
                            "status": "running",
                            "goal": clip_chars(&task.goal, 200),
                            "match_in": "goal",
                        }));
                    }
                }
            }

            // Search completed tasks in store
            let scope_str = args.scope.as_deref().unwrap_or("#main");
            if let Ok(msgs) = store.recent_by_op(scope_str, "Task", 100) {
                for msg in msgs {
                    match &msg.data {
                        crate::bus::MessageData::Task(crate::bus::TaskMsg::Request {
                            goal,
                            task_id,
                            ..
                        }) => {
                            if goal.to_lowercase().contains(&pattern_lower) {
                                matches.push(json!({
                                    "id": task_id,
                                    "goal": clip_chars(goal, 200),
                                    "match_in": "goal",
                                }));
                            }
                        }
                        crate::bus::MessageData::Task(crate::bus::TaskMsg::Result {
                            task_id,
                            summary,
                            ok,
                            ..
                        }) => {
                            if summary.to_lowercase().contains(&pattern_lower) {
                                matches.push(json!({
                                    "id": task_id,
                                    "status": if *ok { "completed" } else { "failed" },
                                    "summary": clip_chars(summary, 200),
                                    "match_in": "result",
                                }));
                            }
                        }
                        _ => {}
                    }
                }
            }

            // Deduplicate by task_id and limit
            let mut seen = std::collections::HashSet::new();
            matches.retain(|m| {
                if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                    seen.insert(id.to_string())
                } else {
                    true
                }
            });
            matches.truncate(limit);

            ok(json!({
                "matches": matches,
                "count": matches.len(),
                "pattern": pattern
            }))
        }

        _ => err(ToolError::invalid_args(format!("unknown tool: {name}"))),
    }
}

fn json_to_toml(v: &serde_json::Value) -> toml::Value {
    match v {
        serde_json::Value::Null => toml::Value::String(String::new()),
        serde_json::Value::Bool(b) => toml::Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                toml::Value::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => toml::Value::String(s.clone()),
        serde_json::Value::Array(arr) => toml::Value::Array(arr.iter().map(json_to_toml).collect()),
        serde_json::Value::Object(obj) => {
            let mut table = toml::Table::new();
            for (k, val) in obj {
                table.insert(k.clone(), json_to_toml(val));
            }
            toml::Value::Table(table)
        }
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
                Scope::main(),
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

            let workspace_root =
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let path = crate::runtime::workspace_mind_memory(&workspace_root);
            let current = match crate::runtime::read_optional_file(&path) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    // One-time migration from legacy DB location.
                    let legacy = store.get_head_ltm("conclave").unwrap_or_default();
                    if !legacy.trim().is_empty() {
                        let _ = crate::runtime::atomic_write_file_0600(&path, legacy.trim());
                        legacy
                    } else {
                        String::new()
                    }
                }
                Err(_) => String::new(),
            };
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
                if let Err(e) = crate::runtime::atomic_write_file_0600(&path, &ltm) {
                    return err(ToolError::io(format!("failed to save LTM file: {e}")));
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
                Ok(()) => {
                    bus.publish(
                        respond::want_added(
                            head_id,
                            Scope::main(),
                            want_id.clone(),
                            args.want.clone(),
                            args.context.clone(),
                            priority.to_string(),
                            "mind",
                            None,
                        )
                        .with_origin(Origin::System),
                    )
                    .await;

                    ok(json!({
                        "want_id": want_id,
                        "priority": priority,
                        "status": "added"
                    }))
                }
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
                Ok(true) => {
                    bus.publish(
                        respond::want_removed(head_id, Scope::main(), args.id.clone(), "removed")
                            .with_origin(Origin::System),
                    )
                    .await;
                    ok(json!({"removed": true}))
                }
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
                Scope::main(),
                &need_id,
                "mind",
                priority,
                &want.want,
                &want.context,
            )
            .with_origin(Origin::System);

            bus.publish(msg).await;

            bus.publish(
                respond::want_promoted(
                    head_id,
                    Scope::main(),
                    args.id.clone(),
                    priority_str.to_string(),
                    Some(need_id.clone()),
                )
                .with_origin(Origin::System),
            )
            .await;

            bus.publish(
                respond::want_removed(head_id, Scope::main(), args.id.clone(), "promoted")
                    .with_origin(Origin::System),
            )
            .await;

            ok(json!({
                "promoted": true,
                "want_id": args.id,
                "need_id": need_id,
                "priority": format!("{:?}", priority)
            }))
        }
        "chat_completion" => {
            let args: ChatCompletionArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            if args.model.trim().is_empty() {
                return err(ToolError::invalid_args("model is required"));
            }
            if args.prompt.trim().is_empty() {
                return err(ToolError::invalid_args("prompt is required"));
            }

            let client = match LlmClient::from_model_id_with_options(
                &args.model,
                args.temperature,
                args.max_tokens,
            ) {
                Ok(c) => c,
                Err(e) => {
                    return err(ToolError {
                        code: "E_MODEL_NOT_FOUND".to_string(),
                        message: format!("failed to create client: {e}"),
                        detail: None,
                    });
                }
            };

            let mut messages = Vec::new();
            if let Some(sys) = &args.system {
                if !sys.trim().is_empty() {
                    messages.push(UnifiedMessage::System(sys.clone()));
                }
            }
            messages.push(UnifiedMessage::User(args.prompt.clone()));

            match client.chat(messages).await {
                Ok(response) => ok(json!({
                    "model": args.model,
                    "response": response
                })),
                Err(e) => err(ToolError {
                    code: "E_LLM_ERROR".to_string(),
                    message: format!("LLM request failed: {e}"),
                    detail: None,
                }),
            }
        }
        _ => err(ToolError::invalid_args(format!("unknown tool: {name}"))),
    }
}

pub async fn exec_hand_tool(
    workspace: &Workspace,
    cwd: &SharedCwd,
    store: &Store,
    ems: Option<&EmsHandle>,
    scope: &str,
    name: &str,
    args_json: &str,
    cancel: Option<CancellationToken>,
) -> String {
    let _ = scope; // Reserved for future kernel syscall routing
    let name = canonical_hand_tool_name(name);

    // Hands are read-only: deny mutating tools even if model hallucinates them
    if !is_hand_tool_allowed(name) {
        return err(ToolError::forbidden(format!(
            "hands cannot execute mutating tool '{}'; only heads can mutate",
            name
        )));
    }

    // Dispatch EMS read-only tools
    if name.starts_with("ems_") {
        return match ems {
            Some(h) => exec_ems_tool(h, name, args_json).await,
            None => err(ToolError::db("EMS service not available")),
        };
    }

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
                let pat = if args.case_sensitive {
                    query.clone()
                } else {
                    format!("(?i){}", query)
                };
                match Regex::new(&pat)
                    .map_err(|e| ToolError::invalid_args(format!("invalid regex: {e}")))
                {
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
                        line.to_ascii_lowercase()
                            .contains(&query.to_ascii_lowercase())
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
            let content = match HostHalFs::default().read_to_string(&full).await {
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
                    if let Err(e) = HostHalFs::default().create_dir_all(parent).await {
                        return err(ToolError::io(format!("mkdir error: {e}")));
                    }
                }
            }

            if !overwrite {
                if HostHalFs::default().exists(&full).await.unwrap_or(false) {
                    return err(ToolError {
                        code: "E_EXISTS".to_string(),
                        message: "file exists and overwrite=false".to_string(),
                        detail: Some(json!({"path": to_rel(workspace.root(), &full)})),
                    });
                }
            }

            if let Err(e) = HostHalFs::default()
                .write(&full, args.content.as_bytes())
                .await
            {
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

            let argv = vec![
                "-p1".to_string(),
                "--no-backup-if-mismatch".to_string(),
                "-r-".to_string(),
            ];

            let output = HostHalProcess::default()
                .run_with_stdin_bytes_bounded(
                    "patch",
                    &argv,
                    &cwd_path,
                    None,
                    None,
                    diff.as_bytes(),
                    256 * 1024,
                    256 * 1024,
                    None,
                )
                .await;

            match output {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    if output.success {
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
            let argv = vec![
                "-U".to_string(),
                context.to_string(),
                a.to_string_lossy().to_string(),
                b.to_string_lossy().to_string(),
            ];

            let out = HostHalProcess::default()
                .run_bounded(
                    "diff",
                    &argv,
                    &cwd_path,
                    None,
                    None,
                    512 * 1024,
                    256 * 1024,
                    cancel.clone(),
                )
                .await;
            match out {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                    if stdout.trim().is_empty() {
                        if output.code == 0 {
                            ok(json!({"diff": ""}))
                        } else {
                            err(ToolError::io(stderr.trim().to_string()))
                        }
                    } else {
                        ok(json!({"diff": clip_chars(&stdout, 20_000)}))
                    }
                }
                Err(crate::hal::process::HalProcessError::Cancelled { .. }) => err(ToolError {
                    code: "E_CANCELLED".to_string(),
                    message: "diff cancelled".to_string(),
                    detail: None,
                }),
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

            let exists = HostHalFs::default().exists(&full).await.unwrap_or(false);
            if exists {
                return ok(json!({"created": false, "path": to_rel(workspace.root(), &full)}));
            }

            let res = if parents {
                HostHalFs::default().create_dir_all(&full).await
            } else {
                HostHalFs::default().create_dir(&full).await
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

            let argv: Vec<String> = args_str.split_whitespace().map(|s| s.to_string()).collect();
            let output = HostHalGit::default().run(&cwd_path, &argv, None).await;

            match output {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let code = output.code;

                    if output.success {
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

            match method.as_str() {
                "GET" | "POST" | "PUT" | "DELETE" => {}
                _ => {
                    return err(ToolError::invalid_args(format!(
                        "unsupported method: {method}"
                    )));
                }
            };

            let mut headers: std::collections::HashMap<String, String> =
                args.headers.clone().unwrap_or_default();
            if let Some(auth) = &args.authorization {
                headers.insert("Authorization".to_string(), auth.clone());
            }
            if let Some(ct) = &args.content_type {
                headers.insert("Content-Type".to_string(), ct.clone());
            }

            const MAX_BODY_SIZE: usize = 256 * 1024;
            let hal = HostHalNet::default();
            let resp = hal
                .http_request(HalHttpRequest {
                    method: method.clone(),
                    url: url.to_string(),
                    headers,
                    body: args.body.as_ref().map(|b| b.as_bytes().to_vec()),
                    timeout: std::time::Duration::from_secs(timeout_secs),
                    max_body_bytes: MAX_BODY_SIZE,
                })
                .await;

            match resp {
                Ok(resp) => {
                    let body_str = String::from_utf8_lossy(&resp.body).to_string();
                    ok(json!({
                        "status": resp.status,
                        "headers": resp.headers,
                        "body": body_str,
                        "truncated": resp.truncated,
                        "body_size": resp.body_size
                    }))
                }
                Err(crate::hal::net::HalNetError::Timeout { .. }) => err(ToolError {
                    code: "E_TIMEOUT".to_string(),
                    message: format!("request timed out after {}s", timeout_secs),
                    detail: None,
                }),
                Err(crate::hal::net::HalNetError::Connect(msg)) => err(ToolError {
                    code: "E_CONNECT".to_string(),
                    message: format!("connection failed: {msg}"),
                    detail: None,
                }),
                Err(e) => err(ToolError::io(format!("request failed: {e}"))),
            }
        }

        "http_get" => {
            // Read-only HTTP GET for hands
            #[derive(Deserialize)]
            struct HttpGetArgs {
                url: String,
                #[serde(default)]
                headers: Option<std::collections::HashMap<String, String>>,
                #[serde(default)]
                authorization: Option<String>,
                #[serde(default)]
                timeout: Option<u64>,
            }

            let args: HttpGetArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            let url = args.url.trim();
            if url.is_empty() {
                return err(ToolError::invalid_args("url is empty"));
            }

            let timeout_secs = args.timeout.unwrap_or(30).min(30);

            let mut headers: std::collections::HashMap<String, String> =
                args.headers.clone().unwrap_or_default();
            if let Some(auth) = &args.authorization {
                headers.insert("Authorization".to_string(), auth.clone());
            }

            const MAX_BODY_SIZE: usize = 256 * 1024;
            let hal = HostHalNet::default();
            let resp = hal
                .http_request(HalHttpRequest {
                    method: "GET".to_string(),
                    url: url.to_string(),
                    headers,
                    body: None,
                    timeout: std::time::Duration::from_secs(timeout_secs),
                    max_body_bytes: MAX_BODY_SIZE,
                })
                .await;

            match resp {
                Ok(resp) => {
                    let body_str = String::from_utf8_lossy(&resp.body).to_string();
                    ok(json!({
                        "status": resp.status,
                        "headers": resp.headers,
                        "body": body_str,
                        "truncated": resp.truncated,
                        "body_size": resp.body_size
                    }))
                }
                Err(crate::hal::net::HalNetError::Timeout { .. }) => err(ToolError {
                    code: "E_TIMEOUT".to_string(),
                    message: format!("request timed out after {}s", timeout_secs),
                    detail: None,
                }),
                Err(crate::hal::net::HalNetError::Connect(msg)) => err(ToolError {
                    code: "E_CONNECT".to_string(),
                    message: format!("connection failed: {msg}"),
                    detail: None,
                }),
                Err(e) => err(ToolError::io(format!("request failed: {e}"))),
            }
        }

        "chat_completion" => {
            let args: ChatCompletionArgs = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
            };

            if args.model.trim().is_empty() {
                return err(ToolError::invalid_args("model is required"));
            }
            if args.prompt.trim().is_empty() {
                return err(ToolError::invalid_args("prompt is required"));
            }

            let client = match LlmClient::from_model_id_with_options(
                &args.model,
                args.temperature,
                args.max_tokens,
            ) {
                Ok(c) => c,
                Err(e) => {
                    return err(ToolError {
                        code: "E_MODEL_NOT_FOUND".to_string(),
                        message: format!("failed to create client: {e}"),
                        detail: None,
                    });
                }
            };

            let mut messages = Vec::new();
            if let Some(sys) = &args.system {
                if !sys.trim().is_empty() {
                    messages.push(UnifiedMessage::System(sys.clone()));
                }
            }
            messages.push(UnifiedMessage::User(args.prompt.clone()));

            match client.chat(messages).await {
                Ok(response) => ok(json!({
                    "model": args.model,
                    "response": response
                })),
                Err(e) => err(ToolError {
                    code: "E_LLM_ERROR".to_string(),
                    message: format!("LLM request failed: {e}"),
                    detail: None,
                }),
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
        File::create(root.join("file1.txt"))
            .unwrap()
            .write_all(b"content1")
            .unwrap();
        File::create(root.join("file2.py"))
            .unwrap()
            .write_all(b"print('hello')")
            .unwrap();
        File::create(root.join("file3.rs"))
            .unwrap()
            .write_all(b"fn main() {}")
            .unwrap();
        fs::create_dir(root.join("subdir")).unwrap();
        File::create(root.join("subdir/nested.txt"))
            .unwrap()
            .write_all(b"nested")
            .unwrap();

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

    #[test]
    fn test_head_tools_include_consult() {
        let tools = head_tool_specs();
        let consult = tools
            .iter()
            .find(|t| t.function.name == "advisor_consult")
            .expect("advisor_consult tool spec missing");

        let props = consult
            .function
            .parameters
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("advisor_consult parameters missing properties");
        assert!(props.contains_key("case"));
        assert!(props.contains_key("visibility"));

        let required = consult
            .function
            .parameters
            .get("required")
            .and_then(|v| v.as_array())
            .expect("advisor_consult parameters missing required");
        assert!(required.iter().any(|v| v.as_str() == Some("case")));
    }

    #[test]
    fn test_hand_tool_effect_classification() {
        // Read-only tools (uses canonical names internally)
        assert_eq!(hand_tool_effect("list_files"), Some(ToolEffect::ReadOnly));
        assert_eq!(hand_tool_effect("read_file"), Some(ToolEffect::ReadOnly));
        assert_eq!(hand_tool_effect("search_files"), Some(ToolEffect::ReadOnly));
        assert_eq!(hand_tool_effect("diff_files"), Some(ToolEffect::ReadOnly));
        assert_eq!(hand_tool_effect("echo"), Some(ToolEffect::ReadOnly));
        assert_eq!(
            hand_tool_effect("chat_completion"),
            Some(ToolEffect::ReadOnly)
        );

        // Mutating tools (classified but blocked for hands)
        assert_eq!(hand_tool_effect("write_file"), Some(ToolEffect::Mutating));
        assert_eq!(hand_tool_effect("apply_patch"), Some(ToolEffect::Mutating));
        assert_eq!(hand_tool_effect("mkdir"), Some(ToolEffect::Mutating));
        assert_eq!(hand_tool_effect("git"), Some(ToolEffect::Mutating));
        assert_eq!(hand_tool_effect("curl"), Some(ToolEffect::Mutating));

        // Unknown tools return None
        assert_eq!(hand_tool_effect("unknown_tool"), None);
    }

    #[test]
    fn test_head_tool_effect_classification() {
        // Read-only tools (uses canonical names internally)
        assert_eq!(head_tool_effect("recall"), Some(ToolEffect::ReadOnly));
        assert_eq!(head_tool_effect("introspect"), Some(ToolEffect::ReadOnly));
        assert_eq!(head_tool_effect("read_file"), Some(ToolEffect::ReadOnly));
        assert_eq!(head_tool_effect("list_files"), Some(ToolEffect::ReadOnly));

        // Mutating tools
        assert_eq!(head_tool_effect("write_file"), Some(ToolEffect::Mutating));
        assert_eq!(head_tool_effect("apply_patch"), Some(ToolEffect::Mutating));
        assert_eq!(head_tool_effect("mkdir"), Some(ToolEffect::Mutating));
        assert_eq!(head_tool_effect("git"), Some(ToolEffect::Mutating));
        assert_eq!(head_tool_effect("curl"), Some(ToolEffect::Mutating));

        // Unknown tools return None
        assert_eq!(head_tool_effect("unknown_tool"), None);
    }

    #[test]
    fn test_hand_tool_allowlist() {
        // Allowed tools (canonical names)
        assert!(is_hand_tool_allowed("list_files"));
        assert!(is_hand_tool_allowed("read_file"));
        assert!(is_hand_tool_allowed("search_files"));
        assert!(is_hand_tool_allowed("diff_files"));
        assert!(is_hand_tool_allowed("echo"));
        assert!(is_hand_tool_allowed("http_get"));
        assert!(is_hand_tool_allowed("chat_completion"));

        // Mutating tools not allowed for hands
        assert!(!is_hand_tool_allowed("write_file"));
        assert!(!is_hand_tool_allowed("apply_patch"));
        assert!(!is_hand_tool_allowed("mkdir"));
        assert!(!is_hand_tool_allowed("git"));
        assert!(!is_hand_tool_allowed("curl"));
    }

    #[test]
    fn test_hand_tool_specs_exclude_mutating_tools() {
        let specs = hand_tool_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.function.name.as_str()).collect();

        // Should NOT include mutating tools (checking both canonical and spec names)
        assert!(!names.contains(&"write_file"));
        assert!(!names.contains(&"fs_write"));
        assert!(!names.contains(&"apply_patch"));
        assert!(!names.contains(&"patch_apply"));
        assert!(!names.contains(&"mkdir"));
        assert!(!names.contains(&"fs_mkdir"));
        assert!(!names.contains(&"git"));
        assert!(!names.contains(&"git_run"));
    }

    #[test]
    fn test_head_tool_specs_include_mutating_tools() {
        let specs = head_tool_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.function.name.as_str()).collect();

        // Should include mutating tools (spec names)
        assert!(names.contains(&"fs_write"));
        assert!(names.contains(&"patch_apply"));
        assert!(names.contains(&"fs_mkdir"));
        assert!(names.contains(&"git_run"));
        assert!(names.contains(&"http_request"));
    }
}
