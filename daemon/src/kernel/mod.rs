//! Kernel - Core syscall runtime and coordination primitives
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The kernel provides the syscall dispatch runtime, frame wire protocol, and
//! coordination primitives for Abbot's distributed agent system.
//!
//! Key modules:
//! - frame: Wire protocol for syscalls and turn streams
//! - dispatcher: Syscall routing and execution with lane-based concurrency
//! - syscall: Syscall trait and execution context
//! - router: Lane assignment for concurrency control
//! - turns: Turn lifecycle and external tool rendezvous (syscall refactor)
//! - sigcall_hub: Outbound frame broadcast (kernel -> client)
//! - needs/tasks: Work queue coordination for heads and hands
//! - rooms: Multi-agent coordination primitives
//!
//! WHY this module exists: Centralizes kernel primitives that enforce syscall-
//! driven semantics from the refactor spec (chat:* syscalls, turn state, actor
//! separation).

pub mod dispatcher;
pub mod error;
pub mod external_tools;
pub mod frame;
pub mod frame_select;
pub mod frame_store;
pub mod needs;
pub mod rooms;
pub mod router;
pub mod sigcall_hub;
pub mod syscall;
pub mod tasks;
pub mod tick;
pub mod turns;

pub use frame_store::{FrameStore, StoredFrame};
pub use dispatcher::{KernelDispatcher, KernelReceiver};
pub use error::KernelError;
pub use external_tools::ExternalToolManager;
pub use frame::{Frame, FrameOp};
pub use frame_select::{build_frame_select_sql, ConversationItem, ConversationNeed, ConversationTask, FrameSelectArgs};
pub use needs::NeedKernel;
pub use rooms::{RoomKernel, RoomKind, RoomRecord};
pub use router::{KernelRouter, Lane};
pub use sigcall_hub::SigcallHub;
pub use syscall::{Syscall, SyscallContext};
pub use tasks::{BatchCall, TaskItem, TaskKernel};
pub use tick::{Tick, TickKernel};
pub use turns::{ExternalToolResult, TurnKey, TurnRuntime, TurnWaitError};
