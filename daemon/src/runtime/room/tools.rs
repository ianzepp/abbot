//! Room Tools - Shared workspace context builder
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Provides `build_workspace_context()` — a shared context builder that assembles
//! a markdown summary of the current workspace state. This includes VFS root listing,
//! recent git history, current branch, and key documentation files (AGENTS.md, README.md).
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Single source of truth**: Both room runner and mind loop bundle use this same
//!   function, ensuring agents receive consistent workspace awareness regardless of
//!   which execution path invokes them.
//! - **Best-effort assembly**: Each context section is gathered independently. Failures
//!   (missing git, empty VFS) are silently skipped rather than propagated — partial
//!   context is better than no context.
//! - **Bounded output**: Documentation files are truncated to 4000 chars to prevent
//!   context window overflow in downstream LLM calls.

use std::collections::BTreeSet;

use crate::vfs::MountTable;

// =============================================================================
// WORKSPACE CONTEXT
// =============================================================================

/// Build workspace context string: VFS root listing, git commits, AGENTS.md, README.md.
///
/// WHY: Agents need situational awareness of the workspace they're operating in.
/// This function assembles a markdown document covering file structure, recent
/// git activity, and project documentation — everything an agent needs to orient
/// itself without manual exploration.
pub async fn build_workspace_context() -> String {
    let mut sections = Vec::new();

    // -------------------------------------------------------------------------
    // PHASE 1: VFS ROOT LISTING
    // WHY: Gives agents a top-level view of available files and mounts.
    // Merges memory-backed entries with host mount prefixes into a single sorted view.
    // -------------------------------------------------------------------------
    let mut entries = BTreeSet::new();

    if let Ok(table) = std::panic::catch_unwind(MountTable::global) {
        // Memory entries at root
        if let Ok(mem_children) = table.memory().list("/").await {
            for child in mem_children {
                let name = child
                    .strip_prefix('/')
                    .unwrap_or(&child)
                    .trim_start_matches('/');
                if !name.is_empty() {
                    entries.insert(name.to_string());
                }
            }
        }

        // Host mount top-level names
        for prefix in table.mount_prefixes() {
            let name = prefix
                .strip_prefix('/')
                .unwrap_or(&prefix)
                .split('/')
                .next()
                .unwrap_or(&prefix);
            if !name.is_empty() {
                entries.insert(name.to_string());
            }
        }
    }

    if !entries.is_empty() {
        let files: Vec<String> = entries.into_iter().collect();
        sections.push(format!(
            "## Workspace Files\n\n```\n{}\n```",
            files.join("\n")
        ));
    } else {
        sections.push("## Workspace Files\n\n(empty)".to_string());
    }

    // -------------------------------------------------------------------------
    // PHASE 2: GIT CONTEXT
    // WHY: Recent commits and current branch give agents understanding of what
    // changed recently and which branch they're operating on. Only reads from
    // the first host mount with a .git directory.
    // -------------------------------------------------------------------------
    if let Ok(table) = std::panic::catch_unwind(MountTable::global) {
        for m in table.host_mounts() {
            let git_dir = m.host_path.join(".git");
            if !git_dir.exists() {
                continue;
            }

            if let Ok(output) = std::process::Command::new("git")
                .args(["log", "--oneline", "-10"])
                .current_dir(&m.host_path)
                .output()
                && output.status.success()
            {
                let commits = String::from_utf8_lossy(&output.stdout);
                let commits = commits.trim();
                if !commits.is_empty() {
                    sections.push(format!(
                        "## Recent Git Commits ({})\n\n```\n{}\n```",
                        m.prefix, commits
                    ));
                }
            }

            if let Ok(output) = std::process::Command::new("git")
                .args(["branch", "--show-current"])
                .current_dir(&m.host_path)
                .output()
                && output.status.success()
            {
                let branch = String::from_utf8_lossy(&output.stdout);
                let branch = branch.trim();
                if !branch.is_empty() {
                    sections.push(format!("## Git Branch ({})\n\n`{}`", m.prefix, branch));
                }
            }

            break; // WHY: Only show git from the first mount with .git
        }

        // ---------------------------------------------------------------------
        // PHASE 3: PROJECT DOCUMENTATION
        // WHY: AGENTS.md and README.md provide project-specific instructions
        // and context that help agents understand the codebase conventions.
        // ---------------------------------------------------------------------
        for m in table.host_mounts() {
            let agents_path = m.host_path.join("AGENTS.md");
            if agents_path.exists()
                && let Ok(content) = std::fs::read_to_string(&agents_path)
            {
                let content = content.trim();
                if !content.is_empty() {
                    sections.push(format!(
                        "## AGENTS.md ({})\n\n{}",
                        m.prefix,
                        truncate_chars(content, 4000)
                    ));
                }
            }

            let readme_path = m.host_path.join("README.md");
            if readme_path.exists()
                && let Ok(content) = std::fs::read_to_string(&readme_path)
            {
                let content = content.trim();
                if !content.is_empty() {
                    sections.push(format!(
                        "## README.md ({})\n\n{}",
                        m.prefix,
                        truncate_chars(content, 4000)
                    ));
                }
            }
        }
    }

    sections.join("\n\n")
}

// =============================================================================
// HELPERS
// =============================================================================

/// Truncate a string to `max_chars` characters, appending an ellipsis if truncated.
///
/// WHY: Prevents unbounded content from blowing up LLM context windows.
/// Uses char count (not byte count) for correct handling of multibyte UTF-8.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...\n\n(truncated)", truncated)
    }
}
