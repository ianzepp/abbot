//! Proc - External process execution with defense-in-depth security
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The `proc` namespace provides controlled execution of external programs (git, cargo,
//! npm, etc.) through the `proc:run` syscall. It integrates with the Hardware Abstraction
//! Layer (HAL) via the `HalProcess` trait and the Virtual Filesystem (VFS) via `MountTable`
//! for path resolution.
//!
//! **Security model:**
//! This namespace implements multiple overlapping security controls to prevent arbitrary
//! code execution, resource exhaustion, and filesystem escapes:
//! - **Allowlist-based filtering**: Only approved programs can execute (no shells)
//! - **Actor-based authorization**: Only "head" agents may execute processes
//! - **Output size limiting**: Bounded stdout/stderr prevents memory exhaustion
//! - **Timeout enforcement**: Hung processes are terminated after deadline
//! - **Cancellation propagation**: Child processes die when parent task cancels
//! - **VFS path validation**: Working directories must be within mounted paths
//!
//! **Registered syscalls:**
//! - `proc:run` - Execute external program with structured arguments and security constraints
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Allowlist over blocklist**: Only known-safe tools run (default-deny)
//! - **No shell access**: Shells (`sh`, `bash`) blocked to prevent command injection
//! - **Defense-in-depth**: Multiple independent security layers
//! - **Actor-based authorization**: "Head" decides, "hand" cannot execute
//! - **Fail-safe defaults**: Conservative timeout and output limits

mod run;

pub use run::ProcRun;
