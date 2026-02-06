//! Git - Git operations with three-tier security controls
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The `git` namespace provides controlled execution of git commands through the `git:run`
//! syscall. It wraps the `proc:run` syscall with git-specific security validation, implementing
//! a three-tier security strategy:
//!
//! 1. **Forbidden commands** - Never allowed (push, config, credential, remote)
//! 2. **Mutating commands** - Require "head" actor authorization (commit, merge, etc.)
//! 3. **Read-only commands** - Allowed for all agents (status, log, diff, etc.)
//!
//! **Security model:**
//! This namespace prevents destructive git operations, credential exposure, and unauthorized
//! repository modifications while enabling safe read operations for all agent types:
//! - **Forbidden command blocklist**: No push, config, credential, or remote operations
//! - **Dangerous flag blocklist**: No --exec or -c flags (prevent code execution)
//! - **Actor-based mutation control**: Only "head" agents may modify repository state
//! - **VFS path validation**: Git operates only within mounted workspaces
//! - **No network operations**: Push/pull/fetch blocked to prevent data exfiltration
//!
//! **Registered syscalls:**
//! - `git:run` - Execute git command with layered security validation
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Layered security**: Three-tier validation (forbidden > mutating > read-only)
//! - **No network operations**: Block push/pull to prevent data exfiltration
//! - **Config immutability**: Git config is read-only (cannot modify behavior)
//! - **Actor-based mutation control**: "Head" modifies, "hand" reads
//! - **Deny-list approach**: Block known-dangerous commands (git has 100+ subcommands)

mod run;

pub use run::GitRun;
