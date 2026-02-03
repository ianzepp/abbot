use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use abbot::runtime::{ProcKind, ProcService};

#[tokio::test]
async fn test_watcher_notified_on_update() {
    let proc = ProcService::new().handle();

    proc.write()
        .await
        .create(ProcKind::Tasks, "abc", json!({"status": "pending"}));

    let notify = proc.write().await.watcher(ProcKind::Tasks, "abc");
    let notified = Arc::new(AtomicBool::new(false));
    let notified_clone = notified.clone();

    let handle = tokio::spawn(async move {
        notify.notified().await;
        notified_clone.store(true, Ordering::SeqCst);
    });

    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!notified.load(Ordering::SeqCst));

    proc.write()
        .await
        .update(ProcKind::Tasks, "abc", json!({"status": "done"}));

    tokio::time::timeout(Duration::from_millis(100), handle)
        .await
        .expect("timeout")
        .expect("join");

    assert!(notified.load(Ordering::SeqCst));
}

#[tokio::test]
async fn test_watcher_notified_on_delete() {
    let proc = ProcService::new().handle();

    proc.write().await.create(ProcKind::Tasks, "abc", json!({}));

    let notify = proc.write().await.watcher(ProcKind::Tasks, "abc");
    let notified = Arc::new(AtomicBool::new(false));
    let notified_clone = notified.clone();

    let handle = tokio::spawn(async move {
        notify.notified().await;
        notified_clone.store(true, Ordering::SeqCst);
    });

    tokio::time::sleep(Duration::from_millis(10)).await;

    proc.write().await.delete(ProcKind::Tasks, "abc");

    tokio::time::timeout(Duration::from_millis(100), handle)
        .await
        .expect("timeout")
        .expect("join");

    assert!(notified.load(Ordering::SeqCst));
}
