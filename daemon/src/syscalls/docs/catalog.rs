use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::hal::llm::ToolSpec;
use crate::syscalls::dispatch::describe_tools;

/// A documentation entry with owned strings (needed for runtime-loaded skills).
pub struct Doc {
    pub name: String,
    pub description: String,
    pub content: String,
}

/// YAML frontmatter for skill documentation files.
#[derive(Deserialize)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub requires: Vec<String>,
}

/// Parse YAML frontmatter delimited by `---` from a markdown string.
///
/// Returns the parsed metadata and the remaining body text.
pub fn parse_frontmatter(content: &str) -> Result<(SkillMeta, String), String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err("missing frontmatter delimiter".into());
    }

    // Skip the opening ---
    let after_open = &trimmed[3..];
    let end = after_open
        .find("\n---")
        .ok_or_else(|| "missing closing frontmatter delimiter".to_string())?;

    let yaml_str = &after_open[..end];
    let body = &after_open[end + 4..]; // skip \n---

    let meta: SkillMeta =
        serde_yaml::from_str(yaml_str).map_err(|e| format!("invalid frontmatter YAML: {e}"))?;

    Ok((meta, body.trim_start_matches('\n').to_string()))
}

// =========================================================================
// Syscall doc builder
// =========================================================================

/// Namespace descriptor: name and the tool specs for that namespace.
struct Namespace {
    name: &'static str,
    description: &'static str,
    specs: Vec<ToolSpec>,
}

/// Build all namespace descriptors with their embedded tool specs.
fn namespaces() -> Vec<Namespace> {
    vec![
        Namespace {
            name: "docs",
            description: "Documentation retrieval and search",
            specs: vec![
                tool_spec!("../docs/list"),
                tool_spec!("../docs/read"),
                tool_spec!("../docs/search"),
            ],
        },
        Namespace {
            name: "ems",
            description: "Entity management (tasks, needs, wants, memories)",
            specs: vec![
                tool_spec!("../ems/list"),
                tool_spec!("../ems/insert"),
                tool_spec!("../ems/select"),
                tool_spec!("../ems/update"),
                tool_spec!("../ems/delete"),
                tool_spec!("../ems/describe"),
            ],
        },
        Namespace {
            name: "exec",
            description: "Shell command execution",
            specs: vec![tool_spec!("../exec/run")],
        },
        Namespace {
            name: "fs",
            description: "Filesystem operations (read, write, list, grep)",
            specs: vec![
                tool_spec!("../fs/cd"),
                tool_spec!("../fs/grep"),
                tool_spec!("../fs/list"),
                tool_spec!("../fs/mkdir"),
                tool_spec!("../fs/read"),
                tool_spec!("../fs/write"),
            ],
        },
        Namespace {
            name: "hand",
            description: "Delegate work to a Hand agent",
            specs: vec![tool_spec!("../hand/run")],
        },
        Namespace {
            name: "llm",
            description: "Direct LLM chat completions",
            specs: vec![tool_spec!("../llm/chat")],
        },
        Namespace {
            name: "need",
            description: "Create needs (pending requirements)",
            specs: vec![tool_spec!("../need/create")],
        },
        Namespace {
            name: "net",
            description: "Network operations (HTTP fetch)",
            specs: vec![tool_spec!("../net/fetch")],
        },
        Namespace {
            name: "noop",
            description: "No-op signals for round/room termination",
            specs: vec![tool_spec!("../noop/done"), tool_spec!("../noop/signal")],
        },
        Namespace {
            name: "patch",
            description: "Apply unified diff patches to files",
            specs: vec![tool_spec!("../patch/apply")],
        },
        Namespace {
            name: "room",
            description: "Multi-agent room coordination",
            specs: vec![tool_spec!("../room/context")],
        },
        Namespace {
            name: "tool",
            description: "Tool introspection and explanation",
            specs: vec![tool_spec!("../tool/explain")],
        },
        Namespace {
            name: "want",
            description: "Manage wants (goals and desires)",
            specs: vec![
                tool_spec!("../want/create"),
                tool_spec!("../want/list"),
                tool_spec!("../want/promote"),
                tool_spec!("../want/remove"),
            ],
        },
    ]
}

/// Build a Doc for a single syscall namespace from its JSON tool specs.
pub fn build_syscall_doc(ns_name: &str) -> Option<Doc> {
    namespaces()
        .into_iter()
        .find(|ns| ns.name == ns_name)
        .map(|ns| {
            let content = format!("# syscalls/{}\n\n{}", ns.name, describe_tools(&ns.specs));
            Doc {
                name: format!("syscalls/{}", ns.name),
                description: ns.description.to_string(),
                content,
            }
        })
}

// =========================================================================
// Skill loader
// =========================================================================

/// Load skill docs from a directory of `*.md` files with YAML frontmatter.
fn load_skills_from_dir(dir: &Path) -> Vec<Doc> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut docs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let (meta, body) = match parse_frontmatter(&raw) {
            Ok(pair) => pair,
            Err(_) => continue, // skip files with bad frontmatter
        };

        docs.push(Doc {
            name: format!("skills/{}", meta.name),
            description: meta.description,
            content: format!("# skills/{}\n\n{}", meta.name, body),
        });
    }
    docs
}

// =========================================================================
// Full catalog builder
// =========================================================================

/// Build the complete documentation catalog.
///
/// 1. Syscall docs — auto-assembled from JSON tool specs (one per namespace)
/// 2. Embedded skills — shipped with binary (from `prompts/skills/`)
/// 3. Global skills — from `~/.abbot/skills/*.md` (override embedded by name)
pub fn build_catalog() -> Vec<Doc> {
    let mut catalog: Vec<Doc> = Vec::new();

    // 1. Syscall docs for all namespaces
    for ns in namespaces() {
        let content = format!("# syscalls/{}\n\n{}", ns.name, describe_tools(&ns.specs));
        catalog.push(Doc {
            name: format!("syscalls/{}", ns.name),
            description: ns.description.to_string(),
            content,
        });
    }

    // 2. Embedded skills (none yet — infrastructure only)
    // Future: add include_str! loaded skills here

    // 3. Global skills from ~/.abbot/skills/
    let mut skill_map: HashMap<String, Doc> = HashMap::new();

    if let Some(config) = crate::runtime::app_config::config_dir() {
        let skills_dir = config.join("skills");
        for doc in load_skills_from_dir(&skills_dir) {
            skill_map.insert(doc.name.clone(), doc);
        }
    }

    catalog.extend(skill_map.into_values());

    // Sort by name for deterministic output
    catalog.sort_by(|a, b| a.name.cmp(&b.name));
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_syscall_doc() {
        let doc = build_syscall_doc("fs").expect("fs namespace should exist");
        assert_eq!(doc.name, "syscalls/fs");
        assert!(doc.content.contains("# syscalls/fs"));
        assert!(doc.content.contains("tool__fs_read"));
    }

    #[test]
    fn test_unknown_namespace_returns_none() {
        assert!(build_syscall_doc("nonexistent").is_none());
    }

    #[test]
    fn test_build_catalog_has_all_namespaces() {
        let catalog = build_catalog();
        let syscall_docs: Vec<&Doc> = catalog
            .iter()
            .filter(|d| d.name.starts_with("syscalls/"))
            .collect();
        assert_eq!(syscall_docs.len(), 13, "expected 13 syscall namespace docs");
    }

    #[test]
    fn test_parse_frontmatter() {
        let input = "---\nname: kubernetes\ndescription: K8s management\n---\nBody text here.";
        let (meta, body) = parse_frontmatter(input).unwrap();
        assert_eq!(meta.name, "kubernetes");
        assert_eq!(meta.description, "K8s management");
        assert_eq!(body, "Body text here.");
    }

    #[test]
    fn test_parse_frontmatter_missing() {
        let input = "No frontmatter here.";
        assert!(parse_frontmatter(input).is_err());
    }
}
