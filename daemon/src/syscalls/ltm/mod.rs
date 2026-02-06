//! LTM - Long-Term Memory Management
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This namespace manages persistent long-term memory (LTM) for agents across kernel
//! sessions. LTM stores important facts, decisions, and context that agents need to
//! maintain continuity between conversations and kernel restarts.
//!
//! **Memory hierarchy in Abbot:**
//! - **STM (Short-Term Memory)**: Ephemeral session state, cleared on restart
//! - **LTM (Long-Term Memory)**: Persistent facts and decisions, survives restarts
//! - **Recall (Vector Memory)**: Semantic search over conversation transcripts
//!
//! **Storage location:**
//! - Primary: Workspace filesystem at `.abbot/workspace/mind/memory.md`
//! - Legacy: SQLite `head_memory` table (migrated on first access)
//!
//! **Integration points:**
//! - `workspace_mind_memory()` returns LTM file path for workspace
//! - `Store::get_head_ltm()` reads legacy database LTM (migration path)
//! - `HeadBundleBuilder` loads LTM into system prompts for "head" agents
//! - `RoomBundleBuilder` loads LTM into room context for collaborative agents
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Persistence over sessions**: LTM survives kernel restarts, unlike STM
//! - **File-based storage**: Markdown file in workspace for easy inspection/editing
//! - **Mutation guarded**: Only "head" agents can modify LTM (prevents corruption)
//! - **Structured operations**: Append/replace/remove ops prevent arbitrary edits
//! - **Workspace-scoped**: Each workspace has independent LTM (prevents cross-contamination)
//!
//! WHY LTM EXISTS
//! ==============
//! Agents need continuity between sessions. Without LTM:
//! - Users must re-explain project context every conversation
//! - Agents forget past decisions and repeat mistakes
//! - No memory of preferences, conventions, or gotchas
//!
//! LTM solves this by storing:
//! - Project architecture decisions ("WHY we use X pattern")
//! - User preferences ("NEVER use emojis in code")
//! - Known issues ("WARNING: avoid Y due to Z bug")
//! - Team conventions ("ALWAYS format with prettier before commit")
//!
//! TRADE-OFFS
//! ==========
//! 1. **File vs. Database Storage**
//!    - CHOSEN: Markdown file at `.abbot/workspace/mind/memory.md`
//!    - REJECTED: SQLite `head_memory` table (legacy)
//!    - WHY: Files are human-readable, git-committable, and easily inspectable
//!    - IMPLICATION: Legacy database LTM is migrated on first access
//!
//! 2. **Structured Ops vs. Free Text**
//!    - CHOSEN: Append/replace/remove operations with patterns
//!    - WHY: Prevents agents from corrupting LTM with arbitrary edits
//!    - IMPLICATION: More verbose API, but safer for programmatic updates
//!
//! 3. **Workspace-Scoped vs. Global**
//!    - CHOSEN: One LTM file per workspace
//!    - WHY: Different projects have different context and conventions
//!    - IMPLICATION: LTM doesn't transfer between workspaces (isolation)
//!
//! CONCURRENCY
//! ===========
//! - LTM updates are serialized via filesystem atomic writes
//! - Read-modify-write pattern in `ltm:update` is safe due to mutation guard
//! - Multiple reads are safe (immutable filesystem access)
//!
//! SECURITY MODEL
//! ==============
//! - Only "head" agents may update LTM (mutation permission required)
//! - "Hand" and "room" agents can read LTM but not modify it
//! - WHY: Prevents compromised LLMs from injecting false context into LTM
//!
//! SYSCALLS
//! ========
//! - `ltm:update` - Modify long-term memory (append/replace/remove operations)

mod update;

pub use update::LtmUpdate;

use crate::kernel::KernelDispatcher;
use std::sync::Arc;

/// Register all LTM syscalls with the kernel dispatcher.
///
/// WHY: Centralized registration ensures consistent syscall availability across
/// all kernel instances. Called during kernel initialization.
pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(LtmUpdate::new()));
}
