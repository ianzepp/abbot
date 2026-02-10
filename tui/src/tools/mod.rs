//! Compiled-in tool specs and executor dispatch for TUI tool execution.

use serde_json::{Value, json};

use crate::ws::Frame;

// =============================================================================
// TOOL RESULT
// =============================================================================

pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

// =============================================================================
// TOOL REGISTRATION
// =============================================================================

/// Build a `tool:register` frame for the initial room.
pub fn registration_frame(room: &str) -> Frame {
    Frame::req(
        "tool:register",
        json!({
            "room": room,
            "tools": tool_specs(),
        }),
    )
    .with_actor("server/tui")
}

fn tool_specs() -> Vec<Value> {
    vec![
        json!({
            "name": "bash",
            "summary": "Execute a shell command",
            "description": "Execute a shell command and return stdout, stderr, and exit code.",
            "schema_json": json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Optional timeout in milliseconds (default: 30000)"
                    }
                },
                "required": ["command"]
            }).to_string()
        }),
        json!({
            "name": "glob",
            "summary": "Find files matching a glob pattern",
            "description": "Find files matching a glob pattern and return matched paths.",
            "schema_json": json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern (e.g. \"src/**/*.rs\")"
                    }
                },
                "required": ["pattern"]
            }).to_string()
        }),
        json!({
            "name": "grep",
            "summary": "Search file contents with regex",
            "description": "Search file contents with a regex pattern and return matching lines with file:line prefix.",
            "schema_json": json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory or file to search in (default: current directory)"
                    },
                    "glob": {
                        "type": "string",
                        "description": "Optional glob filter for file names (e.g. \"*.rs\")"
                    }
                },
                "required": ["pattern"]
            }).to_string()
        }),
        json!({
            "name": "read_file",
            "summary": "Read file contents",
            "description": "Read the contents of a file, with optional line offset and limit.",
            "schema_json": json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (0-based)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read"
                    }
                },
                "required": ["path"]
            }).to_string()
        }),
        json!({
            "name": "write_file",
            "summary": "Write string to file",
            "description": "Write content to a file, creating or overwriting it.",
            "schema_json": json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    }
                },
                "required": ["path", "content"]
            }).to_string()
        }),
        json!({
            "name": "list_dir",
            "summary": "List directory contents",
            "description": "List the contents of a directory with type indicators (d=directory, f=file).",
            "schema_json": json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the directory to list"
                    }
                },
                "required": ["path"]
            }).to_string()
        }),
    ]
}

// =============================================================================
// DISPLAY HELPERS
// =============================================================================

/// Extract a short summary from tool arguments for display.
pub fn tool_summary(name: &str, arguments: &Value) -> String {
    let brief = match name {
        "bash" => arguments.get("command").and_then(|v| v.as_str()),
        "glob" => arguments.get("pattern").and_then(|v| v.as_str()),
        "grep" => arguments.get("pattern").and_then(|v| v.as_str()),
        "read_file" | "write_file" | "list_dir" => arguments.get("path").and_then(|v| v.as_str()),
        _ => None,
    };
    match brief {
        Some(s) if s.len() > 80 => format!("{}...", &s[..77]),
        Some(s) => s.to_string(),
        None => String::new(),
    }
}

// =============================================================================
// EXECUTOR DISPATCH
// =============================================================================

pub async fn execute_tool(name: &str, arguments: &Value) -> ToolResult {
    match name {
        "bash" => exec_bash(arguments).await,
        "glob" => exec_glob(arguments).await,
        "grep" => exec_grep(arguments).await,
        "read_file" => exec_read_file(arguments).await,
        "write_file" => exec_write_file(arguments).await,
        "list_dir" => exec_list_dir(arguments).await,
        _ => ToolResult {
            content: format!("Unknown tool: {name}"),
            is_error: true,
        },
    }
}

// =============================================================================
// PER-TOOL EXECUTORS
// =============================================================================

async fn exec_bash(args: &Value) -> ToolResult {
    let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
    if command.is_empty() {
        return ToolResult {
            content: "command is required".into(),
            is_error: true,
        };
    }

    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(30_000);

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms),
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let exit_code = output.status.code().unwrap_or(-1);
            let content =
                format!("exit_code: {exit_code}\n--- stdout ---\n{stdout}--- stderr ---\n{stderr}");
            ToolResult {
                content,
                is_error: !output.status.success(),
            }
        }
        Ok(Err(e)) => ToolResult {
            content: format!("Failed to execute command: {e}"),
            is_error: true,
        },
        Err(_) => ToolResult {
            content: format!("Command timed out after {timeout_ms}ms"),
            is_error: true,
        },
    }
}

async fn exec_glob(args: &Value) -> ToolResult {
    let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    if pattern.is_empty() {
        return ToolResult {
            content: "pattern is required".into(),
            is_error: true,
        };
    }

    match glob::glob(pattern) {
        Ok(paths) => {
            let mut results = Vec::new();
            for entry in paths {
                match entry {
                    Ok(path) => results.push(path.display().to_string()),
                    Err(e) => results.push(format!("Error: {e}")),
                }
            }
            ToolResult {
                content: if results.is_empty() {
                    "No matches found".into()
                } else {
                    results.join("\n")
                },
                is_error: false,
            }
        }
        Err(e) => ToolResult {
            content: format!("Invalid glob pattern: {e}"),
            is_error: true,
        },
    }
}

async fn exec_grep(args: &Value) -> ToolResult {
    let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    if pattern.is_empty() {
        return ToolResult {
            content: "pattern is required".into(),
            is_error: true,
        };
    }

    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let file_glob = args.get("glob").and_then(|v| v.as_str());

    let re = match regex::Regex::new(pattern) {
        Ok(re) => re,
        Err(e) => {
            return ToolResult {
                content: format!("Invalid regex: {e}"),
                is_error: true,
            };
        }
    };

    let mut results = Vec::new();
    let max_results = 200;

    if let Err(e) = grep_recursive(
        std::path::Path::new(path),
        &re,
        file_glob,
        &mut results,
        max_results,
    ) {
        return ToolResult {
            content: format!("Error searching: {e}"),
            is_error: true,
        };
    }

    ToolResult {
        content: if results.is_empty() {
            "No matches found".into()
        } else {
            results.join("\n")
        },
        is_error: false,
    }
}

fn grep_recursive(
    path: &std::path::Path,
    re: &regex::Regex,
    file_glob: Option<&str>,
    results: &mut Vec<String>,
    max: usize,
) -> std::io::Result<()> {
    if results.len() >= max {
        return Ok(());
    }

    if path.is_dir() {
        let entries = std::fs::read_dir(path)?;
        for entry in entries {
            let entry = entry?;
            let p = entry.path();
            // Skip hidden directories
            if p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            grep_recursive(&p, re, file_glob, results, max)?;
        }
    } else if path.is_file() {
        // Apply glob filter if specified
        if let Some(g) = file_glob {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if let Ok(matcher) = glob::Pattern::new(g)
                && !matcher.matches(name)
            {
                return Ok(());
            }
        }

        // Read and search file
        if let Ok(content) = std::fs::read_to_string(path) {
            for (i, line) in content.lines().enumerate() {
                if results.len() >= max {
                    break;
                }
                if re.is_match(line) {
                    results.push(format!("{}:{}: {}", path.display(), i + 1, line));
                }
            }
        }
    }

    Ok(())
}

async fn exec_read_file(args: &Value) -> ToolResult {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    if path.is_empty() {
        return ToolResult {
            content: "path is required".into(),
            is_error: true,
        };
    }

    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    match tokio::fs::read_to_string(path).await {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let start = offset.min(lines.len());
            let end = if let Some(lim) = limit {
                (start + lim).min(lines.len())
            } else {
                lines.len()
            };
            let slice = &lines[start..end];
            ToolResult {
                content: slice.join("\n"),
                is_error: false,
            }
        }
        Err(e) => ToolResult {
            content: format!("Error reading file: {e}"),
            is_error: true,
        },
    }
}

async fn exec_write_file(args: &Value) -> ToolResult {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    if path.is_empty() {
        return ToolResult {
            content: "path is required".into(),
            is_error: true,
        };
    }

    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");

    match tokio::fs::write(path, content).await {
        Ok(()) => ToolResult {
            content: format!("Wrote {} bytes to {path}", content.len()),
            is_error: false,
        },
        Err(e) => ToolResult {
            content: format!("Error writing file: {e}"),
            is_error: true,
        },
    }
}

async fn exec_list_dir(args: &Value) -> ToolResult {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");

    match tokio::fs::read_dir(path).await {
        Ok(mut entries) => {
            let mut results = Vec::new();
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let indicator = if entry
                    .file_type()
                    .await
                    .map(|ft| ft.is_dir())
                    .unwrap_or(false)
                {
                    "d"
                } else {
                    "f"
                };
                results.push(format!("{indicator} {name}"));
            }
            results.sort();
            ToolResult {
                content: if results.is_empty() {
                    "Empty directory".into()
                } else {
                    results.join("\n")
                },
                is_error: false,
            }
        }
        Err(e) => ToolResult {
            content: format!("Error listing directory: {e}"),
            is_error: true,
        },
    }
}
