use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::agent_tools::{SharedCwd, ToolError, Workspace, err, ok};
use crate::hal::{HalProcess, HostHalProcess};
use crate::llm::ToolSpec;
use crate::runtime::app_config::{workspace_name_from_root, default_config_path};

/// Plugin access level from config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PluginLevel {
    /// Disabled.
    #[default]
    None,
    /// Read-only (safe for hands and heads).
    Read,
    /// Full access (heads can write, hands read-only).
    Write,
}

#[derive(Debug, Clone)]
pub struct PluginManager {
    levels: std::collections::HashMap<String, PluginLevel>,
    builtins: std::collections::HashMap<String, BuiltinPlugin>,
}

#[derive(Debug, Clone)]
pub struct PluginCatalogEntry {
    pub id: String,
    pub tool_name: String,
    pub description: String,
    pub program: String,
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

    #[serde(default)]
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
    /// Create an empty PluginManager with no enabled plugins.
    pub fn empty() -> Self {
        Self {
            levels: std::collections::HashMap::new(),
            builtins: load_builtin_plugins(),
        }
    }

    pub fn load_for_workspace_root(workspace_root: &Path) -> Self {
        let levels = load_plugin_levels().unwrap_or_default();
        let builtins = load_builtin_plugins();

        let enabled: Vec<String> = levels
            .iter()
            .filter(|(_, level)| matches!(level, PluginLevel::Read | PluginLevel::Write))
            .map(|(id, _)| id.clone())
            .collect();

        if !enabled.is_empty() {
            let workspace_name = workspace_name_from_root(workspace_root);
            let mut v = enabled.clone();
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
            tracing::info!(workspace = %workspace_name, plugins = ?known, unknown_plugins = ?unknown, "plugins enabled");
        }

        Self { levels, builtins }
    }

    pub fn enabled_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.levels
            .iter()
            .filter(|(_, level)| matches!(level, PluginLevel::Read | PluginLevel::Write))
            .map(|(id, _)| id.clone())
            .collect();
        v.sort();
        v
    }

    /// Get the configured level for a plugin.
    pub fn plugin_level(&self, id: &str) -> PluginLevel {
        self.levels.get(id).copied().unwrap_or(PluginLevel::None)
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
                program: p.manifest.program.clone(),
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
        cancel: Option<CancellationToken>,
    ) -> String {
        self.exec_tool_for_role(Role::Hand, workspace, cwd, tool_name, args_json, cancel)
            .await
    }

    pub async fn exec_head_tool(
        &self,
        workspace: &Workspace,
        cwd: &SharedCwd,
        tool_name: &str,
        args_json: &str,
    ) -> String {
        self.exec_tool_for_role(Role::Head, workspace, cwd, tool_name, args_json, None)
            .await
    }

    fn role_tool_specs(&self, role: Role) -> Vec<ToolSpec> {
        let mut out = Vec::new();

        for id in self.enabled_ids() {
            let Some(p) = self.builtins.get(&id) else {
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

        for id in self.enabled_ids() {
            let Some(p) = self.builtins.get(&id) else {
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
        cancel: Option<CancellationToken>,
    ) -> String {
        let Some((policy, p)) = self.lookup_by_tool_name_for_role(role, tool_name) else {
            return err(ToolError::invalid_args(format!(
                "unknown tool: {tool_name}"
            )));
        };

        if !policy.exec {
            return err(ToolError {
                code: "E_FORBIDDEN".to_string(),
                message: format!("{} tool execution disabled", p.manifest.tool_name),
                detail: None,
            });
        }

        // Hands cannot execute write-level plugins
        let level = self.plugin_level(&p.manifest.id);
        if matches!(role, Role::Hand) && matches!(level, PluginLevel::Write) {
            return err(ToolError {
                code: "E_FORBIDDEN".to_string(),
                message: format!(
                    "hands cannot execute write-level plugin '{}'; only heads can write",
                    p.manifest.tool_name
                ),
                detail: None,
            });
        }

        exec_command_tool(policy, &p.manifest, workspace, cwd, args_json, cancel).await
    }

    /// Check if a plugin tool is mutating (level = write).
    pub fn is_plugin_mutating(&self, tool_name: &str) -> bool {
        for id in self.enabled_ids() {
            let Some(p) = self.builtins.get(&id) else {
                continue;
            };
            if p.manifest.tool_name == tool_name {
                return matches!(self.plugin_level(&id), PluginLevel::Write);
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
        for id in self.enabled_ids() {
            let Some(p) = self.builtins.get(&id) else {
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

/// Load plugin levels from abbot.toml [plugins] section.
fn load_plugin_levels() -> Option<std::collections::HashMap<String, PluginLevel>> {
    let path = default_config_path()?;
    let s = std::fs::read_to_string(path).ok()?;

    let v: toml::Value = toml::from_str(&s).ok()?;
    let plugins = v.get("plugins")?.as_table()?;

    let mut levels = std::collections::HashMap::new();
    for (id, value) in plugins {
        if let Some(level_str) = value.as_str() {
            let level = match level_str {
                "read" => PluginLevel::Read,
                "write" => PluginLevel::Write,
                _ => PluginLevel::None,
            };
            levels.insert(id.to_string(), level);
        }
    }

    Some(levels)
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
    cancel: Option<CancellationToken>,
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

    let timeout = m.timeout_secs.map(Duration::from_secs);
    let max_out = policy.max_stdout_chars.unwrap_or(50_000);
    let max_err = policy.max_stderr_chars.unwrap_or(5_000);
    let max_out_bytes = max_out.saturating_mul(4);
    let max_err_bytes = max_err.saturating_mul(4);
    let output = HostHalProcess::default()
        .run_bounded(
            &m.program,
            &full_argv,
            &exec_dir,
            None,
            timeout,
            max_out_bytes,
            max_err_bytes,
            cancel,
        )
        .await;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let code = output.code;

            if output.success {
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
            let workspace_name = workspace_name_from_root(workspace.root());
            match e {
                crate::hal::process::HalProcessError::Timeout { timeout, .. } => err(ToolError {
                    code: "E_TIMEOUT".to_string(),
                    message: format!("{} timed out after {:?}", m.program, timeout),
                    detail: None,
                }),
                crate::hal::process::HalProcessError::Cancelled { .. } => err(ToolError {
                    code: "E_CANCELLED".to_string(),
                    message: format!("{} cancelled", m.program),
                    detail: None,
                }),
                _ => err(ToolError::io(format!(
                    "spawn {} (workspace={workspace_name}): {e}",
                    m.program
                ))),
            }
        }
    }
}

fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}
