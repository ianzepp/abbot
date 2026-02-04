use crate::llm::ToolSpec;

use serde_json::json;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::function(
            "head__task_create",
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
            "head__task_list",
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
            "head__task_read",
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
            "head__task_search",
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
            "head__chat_send",
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
            "head__memory_recall",
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
            "head__state_query",
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
            "head__conclave_request",
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
            "head__advisor_consult",
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
            "head__tool_explain",
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
            "head__fs_read_excerpt",
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
            "head__fs_list_brief",
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
            "head__fs_search_goal",
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
            "head__stm_read",
            "Read the head's short-term memory (STM). Returns current working context.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "head__stm_update",
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
            "head__config_read",
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
            "head__config_update",
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
            "head__models_list",
            "List available models that can be used for head/hand/mind.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        ),
        ToolSpec::function(
            "head__llm_chat",
            "Make a one-shot LLM request to any configured model.",
            json!({
                "type": "object",
                "properties": {
                    "model": {
                        "type": "string",
                        "description": "Model ID (e.g., 'anthropic/claude-sonnet-4-20250514' or 'openrouter/openai/gpt-5.2')"
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
    ]
}
