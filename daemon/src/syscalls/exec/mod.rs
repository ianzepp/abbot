//! Exec - External process execution with defense-in-depth security
//!
//! The `exec` namespace provides controlled execution of external programs (git, cargo,
//! npm, gh, etc.) through the `exec:run` syscall. It integrates with the Hardware Abstraction
//! Layer (HAL) via the `HalProcess` trait and the Virtual Filesystem (VFS) via `MountTable`
//! for path resolution.
//!
//! **Security model:**
//! - **Allowlist-based filtering**: Only approved programs can execute (no shells)
//! - **Actor-based authorization**: Only "head" agents may execute processes
//! - **Output size limiting**: Bounded stdout/stderr prevents memory exhaustion
//! - **Timeout enforcement**: Hung processes are terminated after deadline
//! - **Cancellation propagation**: Child processes die when parent task cancels
//! - **VFS path validation**: Working directories must be within mounted paths
//!
//! **Registered syscalls:**
//! - `exec:run` - Execute external program with structured arguments and security constraints

mod run;

pub use run::ExecRun;
