// Session-scoped write locks for serializing mutating operations.
//
// Heads execute in parallel, but mutating operations (file writes, git, etc.)
// within the same scope/channel should be serialized to avoid conflicts.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Guard returned when acquiring a session write lock.
/// Dropping the guard releases the lock.
pub struct SessionWriteGuard {
    _guard: OwnedMutexGuard<()>,
    pub scope: String,
    pub acquired_at: Instant,
}

/// Manager for session-scoped write locks.
/// Each scope (e.g., "main" or "session/<id>") has its own lock.
#[derive(Clone)]
pub struct SessionWriteLocks {
    inner: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl Default for SessionWriteLocks {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionWriteLocks {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Acquire the write lock for a scope.
    /// Returns a guard that releases the lock when dropped.
    pub async fn acquire(&self, scope: &str) -> SessionWriteGuard {
        let start = Instant::now();

        let lock = {
            let mut map = self.inner.lock().await;
            map.entry(scope.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        // Acquire the per-scope lock (this may wait if another head holds it)
        let guard = lock.lock_owned().await;

        let wait_ms = start.elapsed().as_millis();
        if wait_ms > 100 {
            tracing::debug!(
                scope = scope,
                wait_ms = wait_ms,
                "acquired session write lock after waiting"
            );
        }

        SessionWriteGuard {
            _guard: guard,
            scope: scope.to_string(),
            acquired_at: Instant::now(),
        }
    }

    /// Try to acquire the write lock for a scope without waiting.
    /// Returns None if the lock is held by another head.
    pub async fn try_acquire(&self, scope: &str) -> Option<SessionWriteGuard> {
        let lock = {
            let mut map = self.inner.lock().await;
            map.entry(scope.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        match lock.try_lock_owned() {
            Ok(guard) => Some(SessionWriteGuard {
                _guard: guard,
                scope: scope.to_string(),
                acquired_at: Instant::now(),
            }),
            Err(_) => None,
        }
    }
}

impl Drop for SessionWriteGuard {
    fn drop(&mut self) {
        let held_ms = self.acquired_at.elapsed().as_millis();
        if held_ms > 1000 {
            tracing::debug!(
                scope = self.scope,
                held_ms = held_ms,
                "released session write lock after long hold"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn locks_serialize_within_scope() {
        let locks = SessionWriteLocks::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let locks1 = locks.clone();
        let counter1 = counter.clone();
        let h1 = tokio::spawn(async move {
            let _guard = locks1.acquire("main").await;
            let v = counter1.load(Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            counter1.store(v + 1, Ordering::SeqCst);
        });

        let locks2 = locks.clone();
        let counter2 = counter.clone();
        let h2 = tokio::spawn(async move {
            let _guard = locks2.acquire("main").await;
            let v = counter2.load(Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            counter2.store(v + 1, Ordering::SeqCst);
        });

        h1.await.unwrap();
        h2.await.unwrap();

        // If locks worked, counter should be 2 (serialized increments)
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn different_scopes_run_parallel() {
        let locks = SessionWriteLocks::new();
        let start = Instant::now();

        let locks1 = locks.clone();
        let h1 = tokio::spawn(async move {
            let _guard = locks1.acquire("scope-a").await;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        });

        let locks2 = locks.clone();
        let h2 = tokio::spawn(async move {
            let _guard = locks2.acquire("scope-b").await;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        });

        h1.await.unwrap();
        h2.await.unwrap();

        // If scopes are independent, total time should be ~100ms, not ~200ms
        let elapsed = start.elapsed().as_millis();
        assert!(
            elapsed < 180,
            "expected parallel execution, got {}ms",
            elapsed
        );
    }
}
