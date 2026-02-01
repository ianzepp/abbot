use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;
use tokio::process::Command;
use tokio::time;

use crate::agent_tools::{SharedCwd, ToolError, Workspace, err, ok};
use crate::llm::ToolSpec;
use crate::runtime::app_config::{sandbox_dir_from_workspace_root, sandbox_name_from_workspace_root};

#[derive(Debug, Default, Deserialize)]
struct PluginsToml {
    #[serde(default)]
    enabled: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PluginManager {
    enabled: HashSet<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RoleToolPolicy {
    #[serde(default)]
    expose: bool,
    #[serde(default)]
    exec: bool,
    #[serde(default)]
    max_stdout_chars: Option<usize>,
    #[serde(default)]
    max_stderr_chars: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
struct CommandToolManifest {
    id: String,
    tool_name: String,
    description: String,
    program: String,
    #[serde(default)]
    timeout_secs: Option<u64>,

    #[serde(default)]
    #[allow(dead_code)]
    head: RoleToolPolicy,
    #[serde(default)]
    hand: RoleToolPolicy,
}

impl PluginManager {
    pub fn load_for_workspace_root(workspace_root: &Path) -> Self {
        let enabled = load_enabled(workspace_root).unwrap_or_default();
        if !enabled.is_empty() {
            let sandbox = sandbox_name_from_workspace_root(workspace_root)
                .unwrap_or_else(|| "<unknown>".to_string());
            let mut v: Vec<String> = enabled.iter().cloned().collect();
            v.sort();
            tracing::info!(sandbox = %sandbox, plugins = ?v, "plugins enabled");
        }
        Self { enabled }
    }

    pub fn enabled_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.enabled.iter().cloned().collect();
        v.sort();
        v
    }

    pub fn hand_tool_specs(&self) -> Vec<ToolSpec> {
        let mut out = Vec::new();

        // Built-in command-backed plugins.
        if self.enabled.contains("gh") {
            if let Some(m) = gh_manifest() {
                if m.hand.expose {
                    out.push(command_tool_spec(&m));
                }
            }
        }

        out
    }

    pub fn is_enabled_tool_name(&self, tool_name: &str) -> bool {
        match tool_name {
            "gh" => self.enabled.contains("gh") && gh_manifest().map(|m| m.hand.expose).unwrap_or(false),
            _ => false,
        }
    }

    pub async fn exec_hand_tool(
        &self,
        workspace: &Workspace,
        cwd: &SharedCwd,
        tool_name: &str,
        args_json: &str,
    ) -> String {
        match tool_name {
            "gh" if self.enabled.contains("gh") => {
                let Some(m) = gh_manifest() else {
                    return err(ToolError::io("gh plugin manifest failed to load"));
                };
                if !m.hand.exec {
                    return err(ToolError {
                        code: "E_FORBIDDEN".to_string(),
                        message: "gh tool execution disabled for hand".to_string(),
                        detail: None,
                    });
                }
                exec_command_tool(&m.hand, &m, workspace, cwd, args_json).await
            }
            _ => err(ToolError::invalid_args(format!("unknown tool: {tool_name}"))),
        }
    }
}

fn load_enabled(workspace_root: &Path) -> Option<HashSet<String>> {
    let sandbox_dir = sandbox_dir_from_workspace_root(workspace_root)?;
    let path = sandbox_dir.join("plugins.toml");
    let s = std::fs::read_to_string(path).ok()?;
    let cfg: PluginsToml = toml::from_str(&s).ok()?;
    Some(cfg.enabled.into_iter().collect())
}

fn gh_manifest() -> Option<CommandToolManifest> {
    let raw = include_str!("../plugins/gh/plugin.toml");
    let m = toml::from_str::<CommandToolManifest>(raw).ok()?;
    if m.id != "gh" {
        return None;
    }
    Some(m)
}

fn command_tool_spec(m: &CommandToolManifest) -> ToolSpec {
    ToolSpec::function(
        &m.tool_name,
        &m.description,
        json!({
            "type": "object",
            "properties": {
                "argv": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": format!("Arguments to pass to {} (exclude the program name).", m.program)
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional workspace-relative working directory."
                }
            },
            "required": ["argv"],
            "additionalProperties": false
        }),
    )
}

async fn exec_command_tool(
    policy: &RoleToolPolicy,
    m: &CommandToolManifest,
    workspace: &Workspace,
    cwd: &SharedCwd,
    args_json: &str,
) -> String {
    #[derive(Debug, Deserialize)]
    struct Args {
        argv: Vec<String>,
        #[serde(default)]
        cwd: String,
    }

    let args: Args = match serde_json::from_str(args_json) {
        Ok(v) => v,
        Err(e) => return err(ToolError::invalid_args(format!("invalid JSON args: {e}"))),
    };

    if args.argv.is_empty() {
        return err(ToolError::invalid_args("argv is empty"));
    }

    let cwd_path = cwd.lock().unwrap().clone();
    let exec_dir: PathBuf = if args.cwd.trim().is_empty() {
        cwd_path
    } else {
        match workspace.resolve_from_cwd(&cwd_path, args.cwd.trim()) {
            Ok(p) => p,
            Err(e) => return err(e),
        }
    };

    let run = Command::new(&m.program)
        .args(&args.argv)
        .current_dir(exec_dir)
        .output();

    let output = if let Some(secs) = m.timeout_secs {
        match time::timeout(std::time::Duration::from_secs(secs), run).await {
            Ok(res) => res,
            Err(_) => {
                return err(ToolError {
                    code: "E_TIMEOUT".to_string(),
                    message: format!("{} timed out after {}s", m.program, secs),
                    detail: None,
                });
            }
        }
    } else {
        run.await
    };

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let code = output.status.code().unwrap_or(-1);

            let max_out = policy.max_stdout_chars.unwrap_or(50_000);
            let max_err = policy.max_stderr_chars.unwrap_or(5_000);

            if output.status.success() {
                ok(json!({
                    "stdout": clip_chars(stdout.trim(), max_out),
                    "stderr": clip_chars(stderr.trim(), max_err),
                    "code": code
                }))
            } else {
                err(ToolError {
                    code: format!("E_{}", m.tool_name.to_ascii_uppercase()),
                    message: if !stderr.trim().is_empty() {
                        clip_chars(stderr.trim(), 2000)
                    } else {
                        clip_chars(stdout.trim(), 2000)
                    },
                    detail: Some(json!({"code": code})),
                })
            }
        }
        Err(e) => {
            let sandbox = sandbox_name_from_workspace_root(workspace.root())
                .unwrap_or_else(|| "<unknown>".to_string());
            err(ToolError::io(format!(
                "spawn {} (sandbox={sandbox}): {e}",
                m.program
            )))
        }
    }
}

fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}
