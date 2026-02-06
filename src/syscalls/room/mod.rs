//! Room - Multi-agent reflection and collaboration session management
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The room namespace provides syscalls for managing multi-agent deliberation sessions
//! where participants (strategic, creative, critic) engage in structured dialogue to
//! reach consensus on decisions, proposals, and actions. Rooms represent isolated
//! execution contexts with dedicated streams, persistent schedules, and git worktrees.
//!
//! **Three room types:**
//! - **Conclave**: Strategic planning sessions for high-level decisions (control changes,
//!   self-model updates, long-term memory organization). Limited to 5 rounds to prevent
//!   runaway LLM costs while enabling thorough deliberation.
//! - **Autonomy**: Quick reflection sessions triggered by idle detection. Limited to 3
//!   rounds for fast-turnaround self-correction and tactical adjustments.
//! - **Work**: Task-focused collaboration with git worktree isolation. Limited to 10
//!   rounds to accommodate iterative development (code → test → fix cycles).
//!
//! **Room lifecycle:**
//! 1. `room:create` - Allocate room ID and register with RoomCoordinator
//! 2. `room:stream` - Open bidirectional event channel for observing deliberation
//! 3. `room:run` - Execute deliberation loop via RoomRunner, emit events to stream
//! 4. Stream closes automatically on completion (consensus, timeout, or cancellation)
//!
//! **RoomCoordinator integration:**
//! - Coordinator subscribes to `tick:subscribe` and monitors kernel idle state
//! - Slow idle (minutes) → schedule autonomy room
//! - Deep idle (longer) → schedule conclave room
//! - Meth mode (fever:meth) → continuous autonomy rooms on every tick
//! - Reboot epoch changes → schedule init conclave with wake_mode:init
//! - Schedules persist in `room_schedules` SQLite table with status tracking
//!
//! **RoomRunner execution model:**
//! - Round-bounded iteration: Each room type has max_rounds limit
//! - Participant-sequential: Within each round, participants speak in order
//! - Consensus detection: Loop terminates early when all participants vote "done"
//! - Proposal tracking: ProposalTracker aggregates votes across rounds
//! - Decision execution: Fire-and-forget dispatch of approved needs via `need:enqueue`
//!
//! **Lane assignment:**
//! - All room operations execute on the **Room lane** (dedicated concurrency lane)
//! - WHY separate lane: Isolates room overhead from critical agent operations
//! - Room lane has lower priority than Immediate/Need lanes but higher than Task lane
//! - Room:run may spawn long-running LLM queries without blocking other syscalls
//!
//! **Persistence model:**
//! - Room schedules stored in `room_schedules` table with status enum (pending/running/done/cancelled/failed)
//! - Deliberation transcripts stored in `conclaves` table (historical name retained for backward compat)
//! - Room streams are ephemeral (in-memory channels, not persisted)
//! - Frame events emitted to stream include: room_start, participant_turn, proposal_made, vote_cast, consensus_reached, room_end
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Isolation via rooms**: Each room is an independent execution context with its own
//!   stream, worktree (for work rooms), and cancellation token. Rooms cannot interfere
//!   with each other or with agent operations.
//! - **Structured deliberation**: Participants use coordination tools (propose, vote, done)
//!   rather than free-form chat. This enables programmatic consensus detection and
//!   actionable decision extraction.
//! - **Fire-and-forget decisions**: Approved proposals are dispatched as needs but rooms
//!   don't wait for completion. This prevents room execution from blocking indefinitely
//!   on long-running operations (git clones, LLM queries, file processing).
//! - **Cost-bounded exploration**: Round limits prevent runaway LLM costs from indecisive
//!   deliberation. Rooms either reach consensus quickly or time out with partial results.
//! - **Stream observability**: Room:stream enables external monitoring of deliberation
//!   progress without blocking room execution. Streams close automatically on completion.
//!
//! SYSCALL REGISTRATION
//! ====================
//! All room syscalls are registered via `register()` function called during kernel
//! initialization. Registration order doesn't matter (syscalls are stateless).
//!
//! **Available syscalls:**
//! - `room:create` - Allocate new room ID and register with coordinator
//! - `room:stream` - Open bidirectional event stream for room observation
//! - `room:run` - Execute deliberation loop for a room
//! - `room:schedule` - Persist room schedule for future execution
//! - `room:list` - Query room schedules with optional filtering
//! - `room:reschedule` - Update run_after_ms for pending schedule
//! - `room:cancel` - Mark schedule as cancelled (prevents execution)
//! - `room:request` - Composite syscall: create → stream → run in single call
//!
//! CONCURRENCY
//! ===========
//! - Room syscalls are stateless (safe for concurrent execution)
//! - RoomCoordinator manages stream lifecycle (open_stream/close_stream are synchronized)
//! - Multiple rooms can run concurrently on Room lane (bounded by lane concurrency limit)
//! - Room:stream uses tokio channels (mpsc) for lock-free event distribution
//! - Room:run spawns detached tasks for LLM queries (doesn't block lane)
//!
//! SECURITY MODEL
//! ==============
//! - Room creation requires no special permissions (any actor can create rooms)
//! - Room execution uses system actor (bypasses per-call permission checks)
//! - WHY permissive: Rooms are internal coordination mechanisms, not exposed to external
//!   callers. The RoomCoordinator controls when rooms run based on idle detection.
//! - Worktree isolation prevents work rooms from modifying main workspace
//! - Room streams are in-memory only (no filesystem exposure)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Sequential participants vs parallel**: Participants speak sequentially within
//!    each round so each sees prior responses. This increases latency but enables
//!    richer deliberation (participants react to each other's proposals).
//!
//! 2. **Fire-and-forget decisions vs synchronous execution**: Approved proposals are
//!    dispatched as needs without waiting for completion. This prevents rooms from
//!    blocking indefinitely but means rooms can't react to decision outcomes.
//!
//! 3. **Round limits vs unbounded iteration**: Each room type has a hard max_rounds
//!    limit to prevent runaway LLM costs. This means some deliberations may time out
//!    before reaching consensus, but it guarantees bounded resource consumption.
//!
//! 4. **Ephemeral streams vs persistent logs**: Room streams are in-memory channels
//!    that close on room completion. Callers must consume events in real-time or lose
//!    them. Deliberation transcripts ARE persisted to SQLite for historical analysis.

mod cancel;
mod create;
mod list;
mod request;
mod reschedule;
mod run;
mod schedule;
mod stream;

pub use cancel::RoomCancel;
pub use create::RoomCreate;
pub use list::RoomList;
pub use request::RoomRequest;
pub use reschedule::RoomReschedule;
pub use run::RoomRun;
pub use schedule::RoomSchedule;
pub use stream::RoomStream;

/// Register all room syscalls with the kernel dispatcher.
///
/// WHY: Centralizes syscall registration so kernel initialization can enable the
/// entire room namespace with a single function call. Registration order doesn't
/// matter since syscalls are stateless.
///
/// USAGE: Called once during kernel initialization (see kernel::builder).
pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(RoomCreate::new()));
    dispatcher.register(Arc::new(RoomStream::new()));
    dispatcher.register(Arc::new(RoomRun::new()));
    dispatcher.register(Arc::new(RoomSchedule::new()));
    dispatcher.register(Arc::new(RoomList::new()));
    dispatcher.register(Arc::new(RoomReschedule::new()));
    dispatcher.register(Arc::new(RoomCancel::new()));
    dispatcher.register(Arc::new(RoomRequest::new()));
}
