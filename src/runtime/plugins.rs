use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;
use tokio::process::Command;
use tokio::time;

use crate::agent_tools::{SharedCwd, ToolEffect, ToolError, Workspace, err, ok};
use crate::llm::ToolSpec;
use crate::runtime::app_config::{sandbox_dir_from_workspace_root, sandbox_name_from_workspace_root};



#[derive(Debug, Clone)]
pub struct PluginManager {
    enabled: HashSet<String>,
    builtins: std::collections::HashMap<String, BuiltinPlugin>,
}

#[derive(Debug, Clone)]
pub struct PluginCatalogEntry {
    pub id: String,
    pub tool_name: String,
    pub description: String,
    pub head_expose: bool,
    pub head_exec: bool,
    pub hand_expose: bool,
    pub hand_exec: bool,
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

/// Tool effect for plugins (read or write).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PluginEffect {
    /// Read-only plugin (safe for hands).
    Read,
    /// Mutating plugin (heads only, requires session lock).
    #[default]
    Write,
}

impl From<PluginEffect> for ToolEffect {
    fn from(pe: PluginEffect) -> Self {
        match pe {
            PluginEffect::Read => ToolEffect::ReadOnly,
            PluginEffect::Write => ToolEffect::Mutating,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct CommandToolManifest {
    id: String,
    tool_name: String,
    description: String,
    program: String,
    #[serde(default)]
    args_prefix: Vec<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,

    /// Tool effect classification. Default: write (conservative).
    #[serde(default)]
    effect: PluginEffect,

    #[serde(default)]
    #[allow(dead_code)]
    head: RoleToolPolicy,
    #[serde(default)]
    hand: RoleToolPolicy,
}

#[derive(Debug, Clone)]
struct BuiltinPlugin {
    manifest: CommandToolManifest,
    hand_md: &'static str,
    head_md: &'static str,
}

impl PluginManager {
    pub fn load_for_workspace_root(workspace_root: &Path) -> Self {
        let enabled = load_enabled(workspace_root).unwrap_or_default();
        let builtins = load_builtin_plugins();

        if !enabled.is_empty() {
            let sandbox = sandbox_name_from_workspace_root(workspace_root)
                .unwrap_or_else(|| "<unknown>".to_string());
            let mut v: Vec<String> = enabled.iter().cloned().collect();
            v.sort();
            let known: Vec<String> = v
                .iter()
                .filter(|id| builtins.contains_key(*id))
                .cloned()
                .collect();
            let unknown: Vec<String> = v
                .iter()
                .filter(|id| !builtins.contains_key(*id))
                .cloned()
                .collect();
            tracing::info!(sandbox = %sandbox, plugins = ?known, unknown_plugins = ?unknown, "plugins enabled");
        }

        Self { enabled, builtins }
    }

    pub fn enabled_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.enabled.iter().cloned().collect();
        v.sort();
        v
    }

    pub fn catalog(&self) -> Vec<PluginCatalogEntry> {
        let mut ids: Vec<&String> = self.builtins.keys().collect();
        ids.sort();

        let mut out = Vec::new();
        for id in ids {
            let p = &self.builtins[id];
            out.push(PluginCatalogEntry {
                id: p.manifest.id.clone(),
                tool_name: p.manifest.tool_name.clone(),
                description: p.manifest.description.clone(),
                head_expose: p.manifest.head.expose,
                head_exec: p.manifest.head.exec,
                hand_expose: p.manifest.hand.expose,
                hand_exec: p.manifest.hand.exec,
            });
        }
        out
    }

    pub fn hand_tool_specs(&self) -> Vec<ToolSpec> {
        self.role_tool_specs(Role::Hand)
    }

    pub fn head_tool_specs(&self) -> Vec<ToolSpec> {
        self.role_tool_specs(Role::Head)
    }

    pub fn hand_playbooks_md(&self) -> String {
        self.role_playbooks_md(Role::Hand)
    }

    pub fn head_playbooks_md(&self) -> String {
        self.role_playbooks_md(Role::Head)
    }

    pub fn is_enabled_tool_name(&self, tool_name: &str) -> bool {
        self.is_enabled_tool_name_for_role(Role::Hand, tool_name)
    }

    pub fn is_enabled_head_tool_name(&self, tool_name: &str) -> bool {
        self.is_enabled_tool_name_for_role(Role::Head, tool_name)
    }

    pub async fn exec_hand_tool(
        &self,
        workspace: &Workspace,
        cwd: &SharedCwd,
        tool_name: &str,
        args_json: &str,
    ) -> String {
        self.exec_tool_for_role(Role::Hand, workspace, cwd, tool_name, args_json)
            .await
    }

    pub async fn exec_head_tool(
        &self,
        workspace: &Workspace,
        cwd: &SharedCwd,
        tool_name: &str,
        args_json: &str,
    ) -> String {
        self.exec_tool_for_role(Role::Head, workspace, cwd, tool_name, args_json)
            .await
    }

    fn role_tool_specs(&self, role: Role) -> Vec<ToolSpec> {
        let mut out = Vec::new();

        for id in &self.enabled {
            let Some(p) = self.builtins.get(id) else {
                continue;
            };
            let policy = role_policy(role, &p.manifest);
            if !policy.expose {
                continue;
            }
            out.push(command_tool_spec(&p.manifest));
        }

        out
    }

    fn role_playbooks_md(&self, role: Role) -> String {
        let mut sections: Vec<(String, String)> = Vec::new();

        for id in &self.enabled {
            let Some(p) = self.builtins.get(id) else {
                continue;
            };
            let policy = role_policy(role, &p.manifest);
            if !policy.expose {
                continue;
            }
            let raw = match role {
                Role::Hand => p.hand_md,
                Role::Head => p.head_md,
            };
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            sections.push((p.manifest.tool_name.clone(), clip_chars(trimmed, 4000)));
        }

        if sections.is_empty() {
            return String::new();
        }

        sections.sort_by(|a, b| a.0.cmp(&b.0));

        let mut out = String::new();
        out.push_str("## Tool Playbooks\n\n");
        for (tool, md) in sections {
            out.push_str("### ");
            out.push_str(tool.trim());
            out.push_str("\n\n");
            out.push_str(md.trim());
            out.push_str("\n\n");
        }
        out
    }

    fn is_enabled_tool_name_for_role(&self, role: Role, tool_name: &str) -> bool {
        self.lookup_by_tool_name_for_role(role, tool_name)
            .map(|(policy, _p)| policy.expose)
            .unwrap_or(false)
    }

    async fn exec_tool_for_role(
        &self,
        role: Role,
        workspace: &Workspace,
        cwd: &SharedCwd,
        tool_name: &str,
        args_json: &str,
    ) -> String {
        let Some((policy, p)) = self.lookup_by_tool_name_for_role(role, tool_name) else {
            return err(ToolError::invalid_args(format!("unknown tool: {tool_name}")));
        };

        if !policy.exec {
            return err(ToolError {
                code: "E_FORBIDDEN".to_string(),
                message: format!("{} tool execution disabled", p.manifest.tool_name),
                detail: None,
            });
        }

        // Hands cannot execute write plugins
        if matches!(role, Role::Hand) && p.manifest.effect == PluginEffect::Write {
            return err(ToolError {
                code: "E_FORBIDDEN".to_string(),
                message: format!(
                    "hands cannot execute mutating plugin '{}'; only heads can mutate",
                    p.manifest.tool_name
                ),
                detail: None,
            });
        }

        exec_command_tool(policy, &p.manifest, workspace, cwd, args_json).await
    }

    /// Check if a plugin tool is mutating (effect = write).
    pub fn is_plugin_mutating(&self, tool_name: &str) -> bool {
        for id in &self.enabled {
            let Some(p) = self.builtins.get(id) else {
                continue;
            };
            if p.manifest.tool_name == tool_name {
                return p.manifest.effect == PluginEffect::Write;
            }
        }
        // Unknown plugins are conservatively treated as mutating
        true
    }

    fn lookup_by_tool_name_for_role(
        &self,
        role: Role,
        tool_name: &str,
    ) -> Option<(&RoleToolPolicy, &BuiltinPlugin)> {
        for id in &self.enabled {
            let Some(p) = self.builtins.get(id) else {
                continue;
            };
            if p.manifest.tool_name != tool_name {
                continue;
            }
            let policy = role_policy(role, &p.manifest);
            return Some((policy, p));
        }
        None
    }
}

#[derive(Debug, Clone, Copy)]
enum Role {
    Hand,
    Head,
}

fn role_policy(role: Role, m: &CommandToolManifest) -> &RoleToolPolicy {
    match role {
        Role::Hand => &m.hand,
        Role::Head => &m.head,
    }
}

fn load_enabled(workspace_root: &Path) -> Option<HashSet<String>> {
    let sandbox_dir = sandbox_dir_from_workspace_root(workspace_root)?;
    let path = sandbox_dir.join("plugins.toml");
    let s = std::fs::read_to_string(path).ok()?;

    let v: toml::Value = toml::from_str(&s).ok()?;
    let table = v.as_table()?;
    let mut enabled = HashSet::new();

    // Legacy format: enabled = ["gh", ...]
    if let Some(list) = table.get("enabled").and_then(|v| v.as_array()) {
        for item in list {
            if let Some(id) = item.as_str() {
                enabled.insert(id.to_string());
            }
        }
    }

    // Current format: [pluginname] enabled = true
    for (name, value) in table {
        if let Some(section) = value.as_table() {
            if section.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
                enabled.insert(name.to_string());
            }
        }
    }

    Some(enabled)
}

fn load_builtin_plugins() -> std::collections::HashMap<String, BuiltinPlugin> {
    let mut out = std::collections::HashMap::new();

    fn add_builtin(
        out: &mut std::collections::HashMap<String, BuiltinPlugin>,
        expected_id: &str,
        manifest_raw: &'static str,
        hand_md: &'static str,
        head_md: &'static str,
    ) {
        if let Ok(m) = toml::from_str::<CommandToolManifest>(manifest_raw) {
            if m.id != expected_id {
                return;
            }
            out.insert(
                m.id.clone(),
                BuiltinPlugin {
                    manifest: m,
                    hand_md,
                    head_md,
                },
            );
        }
    }

    include!(concat!(env!("OUT_DIR"), "/builtin_plugins.rs"));

    out
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
                    "description": format!("Arguments to pass to {} (exclude the program name).", m.tool_name)
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

    if args.argv.is_empty() && m.args_prefix.is_empty() {
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

    let mut full_argv = Vec::new();
    full_argv.extend(m.args_prefix.iter().cloned());
    full_argv.extend(args.argv.into_iter());

    let run = Command::new(&m.program)
        .args(&full_argv)
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
