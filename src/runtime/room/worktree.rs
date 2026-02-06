//! Worktree Manager - Git worktree provisioning for work rooms
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Each work room gets an isolated git worktree on a dedicated branch so that
//! code changes don't interfere with the main working tree. The manager handles
//! provisioning (creating branch + worktree) and cleanup (removing worktree).
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Isolation by default: Work rooms operate in `.abbot/worktrees/<room_id>`,
//!   completely separate from the main working tree.
//! - Branch per room: Each worktree gets a `room/<room_id>` branch, enabling
//!   git-based review of room changes after completion.
//! - Idempotent provisioning: If the worktree already exists, provision()
//!   returns the existing path without error.
//!
//! TRADE-OFFS
//! ==========
//! - Uses `std::process::Command` (blocking) rather than async git operations.
//!   Acceptable because worktree operations are fast and infrequent (once per
//!   room lifecycle), and there's no async git library in the dependency tree.
//! - Force-removes worktrees on cleanup, which may discard uncommitted changes.
//!   Acceptable because work rooms should commit before signaling done.

use std::path::PathBuf;

// =============================================================================
// WORKTREE MANAGER
// =============================================================================

/// Provisions and cleans up git worktrees for work rooms.
///
/// WHY this exists: Work rooms need filesystem isolation to prevent concurrent
/// code changes from interfering with each other or the main working tree.
/// Git worktrees provide this isolation with full git history access.
pub struct WorktreeManager {
    workspace_root: PathBuf,
}

impl WorktreeManager {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    /// Provision a worktree for a room.
    ///
    /// Creates a new branch `room/<room_id>` and a worktree at
    /// `.abbot/worktrees/<room_id>`. If the worktree already exists,
    /// returns the existing path (idempotent).
    ///
    /// WHY branch fallback: If the branch already exists (e.g., from a
    /// previous room that wasn't fully cleaned up), retries without `-b`
    /// to attach to the existing branch rather than failing.
    pub fn provision(&self, room_id: &str, branch: Option<&str>) -> Result<PathBuf, String> {
        let worktree_dir = self.worktree_path(room_id);
        if worktree_dir.exists() {
            return Ok(worktree_dir);
        }

        if let Some(parent) = worktree_dir.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create worktree parent dir: {}", e))?;
        }

        let branch_name = branch
            .map(|b| b.to_string())
            .unwrap_or_else(|| format!("room/{}", room_id));

        // -------------------------------------------------------------------------
        // ATTEMPT 1: Create new branch + worktree
        // WHY -b flag: Creates the branch atomically with the worktree, ensuring
        // no orphaned branches if worktree creation fails.
        // -------------------------------------------------------------------------
        let output = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                &branch_name,
                worktree_dir.to_string_lossy().as_ref(),
            ])
            .current_dir(&self.workspace_root)
            .output()
            .map_err(|e| format!("failed to run git worktree add: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);

            // -------------------------------------------------------------------------
            // ATTEMPT 2: Attach to existing branch
            // WHY fallback: Branch may exist from a previous room that crashed before
            // cleanup. Reusing the branch is safer than failing the room entirely.
            // -------------------------------------------------------------------------
            if stderr.contains("already exists") {
                let output2 = std::process::Command::new("git")
                    .args([
                        "worktree",
                        "add",
                        worktree_dir.to_string_lossy().as_ref(),
                        &branch_name,
                    ])
                    .current_dir(&self.workspace_root)
                    .output()
                    .map_err(|e| format!("failed to run git worktree add: {}", e))?;

                if !output2.status.success() {
                    let stderr2 = String::from_utf8_lossy(&output2.stderr);
                    return Err(format!("git worktree add failed: {}", stderr2));
                }
            } else {
                return Err(format!("git worktree add failed: {}", stderr));
            }
        }

        Ok(worktree_dir)
    }

    /// Clean up a worktree for a room.
    ///
    /// WHY --force: The worktree may have uncommitted changes if the room
    /// timed out before committing. Force-remove ensures cleanup always
    /// succeeds. If git remove fails entirely, falls back to rm -rf.
    pub fn cleanup(&self, room_id: &str) -> Result<(), String> {
        let worktree_dir = self.worktree_path(room_id);
        if !worktree_dir.exists() {
            return Ok(());
        }

        let output = std::process::Command::new("git")
            .args([
                "worktree",
                "remove",
                "--force",
                worktree_dir.to_string_lossy().as_ref(),
            ])
            .current_dir(&self.workspace_root)
            .output()
            .map_err(|e| format!("failed to run git worktree remove: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(room_id = %room_id, error = %stderr, "failed to remove worktree, cleaning up manually");
            // WHY manual fallback: git worktree remove can fail if the worktree
            // is corrupt or locked. Removing the directory ensures no stale state.
            let _ = std::fs::remove_dir_all(&worktree_dir);
        }

        Ok(())
    }

    /// Get the worktree path for a room.
    ///
    /// WHY `.abbot/worktrees/`: Keeps worktrees inside the workspace's .abbot
    /// directory, which is already gitignored and owned by abbot.
    pub fn worktree_path(&self, room_id: &str) -> PathBuf {
        self.workspace_root
            .join(".abbot")
            .join("worktrees")
            .join(room_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_path_format() {
        let mgr = WorktreeManager::new("/tmp/workspace");
        let path = mgr.worktree_path("abc-123");
        assert_eq!(
            path,
            PathBuf::from("/tmp/workspace/.abbot/worktrees/abc-123")
        );
    }
}
