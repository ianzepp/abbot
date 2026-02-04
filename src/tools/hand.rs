use crate::llm::ToolSpec;

use serde_json::json;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::function(
            "hand__fs_list",
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
            "hand__fs_search",
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
            "hand__fs_read",
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
            "hand__fs_diff",
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
            "hand__text_echo",
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
            "hand__http_get",
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
            "hand__llm_chat",
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
    ]
}
