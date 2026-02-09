//! RoomRegistry - In-memory cache of named rooms backed by FrameStore.
//!
//! Named rooms (#main, #session/1234) are registry-managed, rehydratable from
//! FrameStore, and GC'd when idle. Anonymous rooms (NeedService) are one-shot
//! and bypass the registry entirely.
//!
//! The registry is the coordination point between external channels (via Door)
//! and room execution. It manages room lifecycle: creation, message injection,
//! door attachment/detachment, and idle garbage collection.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, Notify, RwLock};

use crate::hal::llm::{ChatMessage, Role};
use crate::runtime::room::door::Door;
use crate::runtime::{Room, RoomRunner};

/// In-memory cache of named rooms.
pub struct RoomRegistry {
    rooms: RwLock<HashMap<String, Arc<ActiveRoom>>>,
}

/// A live room managed by the registry.
pub struct ActiveRoom {
    /// Mutable room state (agents, transcript, door).
    pub room: Mutex<Room>,
    /// Runner for executing agent rounds.
    pub runner: RoomRunner,
    /// Last time this room had activity (for GC).
    pub last_activity: Mutex<Instant>,
    /// Wake signal: notify when a new message arrives or door is attached.
    pub notify: Notify,
    /// Completion signal: notify when room processing is done for this turn.
    pub done: Notify,
    /// The scope this room is registered under.
    pub scope: String,
}

impl RoomRegistry {
    pub fn new() -> Self {
        Self {
            rooms: RwLock::new(HashMap::new()),
        }
    }

    /// Get or create a named room for the given scope.
    ///
    /// If the room doesn't exist, creates it with the provided agent config
    /// and starts the runner loop. Returns the active room handle.
    pub async fn get_or_create(
        &self,
        scope: &str,
        create_fn: impl FnOnce() -> (Room, RoomRunner),
    ) -> Arc<ActiveRoom> {
        // Fast path: room already exists
        {
            let rooms = self.rooms.read().await;
            if let Some(active) = rooms.get(scope) {
                *active.last_activity.lock().await = Instant::now();
                return active.clone();
            }
        }

        // Slow path: create room
        let mut rooms = self.rooms.write().await;
        // Double-check after acquiring write lock
        if let Some(active) = rooms.get(scope) {
            *active.last_activity.lock().await = Instant::now();
            return active.clone();
        }

        let (room, runner) = create_fn();
        let active = Arc::new(ActiveRoom {
            room: Mutex::new(room),
            runner,
            last_activity: Mutex::new(Instant::now()),
            notify: Notify::new(),
            done: Notify::new(),
            scope: scope.to_string(),
        });

        rooms.insert(scope.to_string(), active.clone());

        // Start the persistent runner loop for this room
        let active_clone = active.clone();
        tokio::spawn(async move {
            run_persistent_room(active_clone).await;
        });

        active
    }

    /// Inject a user message into an active room and wake it.
    pub async fn inject_message(&self, scope: &str, content: String) {
        let rooms = self.rooms.read().await;
        let Some(active) = rooms.get(scope) else {
            tracing::warn!(scope = scope, "inject_message: room not found");
            return;
        };

        {
            let mut room = active.room.lock().await;
            // Add user message to all agents' histories
            for agent in &mut room.agents {
                agent
                    .messages
                    .push(ChatMessage::new(Role::User, content.clone()));
            }
        }

        *active.last_activity.lock().await = Instant::now();
        active.notify.notify_one();
    }

    /// Attach a door to an active room, resetting state for a new turn.
    ///
    /// Each user message builds a fresh Door (with a new thread_id), so
    /// attach_door is the natural "new turn" boundary. We reset the room
    /// transcript, reactivate all agents, and swap external tools.
    pub async fn attach_door(&self, scope: &str, door: Arc<dyn Door>) {
        let rooms = self.rooms.read().await;
        let Some(active) = rooms.get(scope) else {
            tracing::warn!(scope = scope, "attach_door: room not found");
            return;
        };

        {
            let mut room = active.room.lock().await;

            // Reset for new turn
            room.transcript.clear();
            room.door = Some(door.clone());

            for agent in &mut room.agents {
                agent.active = true;
                // Remove old external tools (user__* prefix), then add new ones
                agent
                    .tools
                    .retain(|t| !t.function.name.starts_with("user__"));
                agent.tools.extend(door.external_tools().iter().cloned());
            }
        }

        *active.last_activity.lock().await = Instant::now();
    }

    /// Detach the door from an active room.
    pub async fn detach_door(&self, scope: &str) {
        let rooms = self.rooms.read().await;
        let Some(active) = rooms.get(scope) else {
            return;
        };

        let mut room = active.room.lock().await;
        room.door = None;
    }

    /// Wait for the room to finish processing the current turn.
    pub async fn wait_for_done(&self, scope: &str) {
        let active = {
            let rooms = self.rooms.read().await;
            rooms.get(scope).cloned()
        };
        if let Some(active) = active {
            active.done.notified().await;
        }
    }

    /// Garbage collect idle rooms past the given timeout.
    /// Returns the number of rooms evicted.
    pub async fn gc_idle(&self, timeout: std::time::Duration) -> usize {
        let mut rooms = self.rooms.write().await;
        let now = Instant::now();
        let mut to_remove = Vec::new();

        for (scope, active) in rooms.iter() {
            let last = *active.last_activity.lock().await;
            if now.duration_since(last) > timeout {
                to_remove.push(scope.clone());
            }
        }

        let count = to_remove.len();
        for scope in to_remove {
            tracing::info!(scope = %scope, "GC: evicting idle room");
            rooms.remove(&scope);
        }

        count
    }

    /// Get the number of active rooms.
    pub async fn len(&self) -> usize {
        self.rooms.read().await.len()
    }

    /// Check if the registry has no active rooms.
    pub async fn is_empty(&self) -> bool {
        self.rooms.read().await.is_empty()
    }
}

impl Default for RoomRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Persistent runner loop for registry-managed rooms.
///
/// Instead of terminating on quiescence, waits for new input via notify.
/// The room only terminates when GC evicts it (the Arc is dropped).
async fn run_persistent_room(active: Arc<ActiveRoom>) {
    loop {
        // Wait for a wake signal (new message, door attached, etc.)
        active.notify.notified().await;

        // Run the room for one turn
        let mut room = active.room.lock().await;
        let _summary = active.runner.run(&mut room, None).await;
        drop(room);

        // Signal that this turn is complete
        active.done.notify_waiters();

        // Update activity timestamp
        *active.last_activity.lock().await = Instant::now();
    }
}
