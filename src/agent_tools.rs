use crate::bus::{Origin, Scope, respond};
use crate::history::Store;
use crate::llm::{ToolSpec};
use crate::memory::Search;
use crate::runtime::RuntimeBus;

use globset::{Glob, GlobSet, GlobSetBuilder};
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

        let raw = Path::new(path);
        let joined = if raw.is_absolute() {
            return Err(ToolError::outside_workspace("absolute paths are not allowed"));
        } else {
            cwd.join(raw)
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
    ]
}

pub fn mind_tool_specs() -> Vec<ToolSpec> {
    vec![ToolSpec::function(
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
    )]
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

pub async fn exec_head_tool(
    bus: &RuntimeBus,
    store: &Store,
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
        _ => err(ToolError::invalid_args(format!("unknown tool: {name}"))),
    }
}

pub async fn exec_mind_tool(
    store: &Store,
    head_id: &str,
    name: &str,
    args_json: &str,
) -> String {
    match name {
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
        _ => err(ToolError::invalid_args(format!("unknown tool: {name}"))),
    }
}

pub async fn exec_hand_tool(
    workspace: &Workspace,
    cwd: &SharedCwd,
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

            let mut builder = GlobSetBuilder::new();
            if !args.pattern.trim().is_empty() {
                let glob = Glob::new(args.pattern.trim())
                    .map_err(|e| ToolError::invalid_args(format!("invalid pattern: {e}")));
                let glob = match glob {
                    Ok(g) => g,
                    Err(e) => return err(e),
                };
                builder.add(glob);
            }
            let matcher: Option<GlobSet> = builder.build().ok();

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
