use std::sync::Once;

use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use abbot::kernel::{Frame, FrameOp, KernelDispatcher};
use abbot::syscalls;
use abbot::vfs::MountTable;

static INIT_MOUNT: Once = Once::new();

fn init_mounts(tmp: &TempDir) {
    INIT_MOUNT.call_once(|| {
        let _ = MountTable::init(vec![], tmp.path().to_path_buf());
    });
}

fn setup_dispatcher() -> KernelDispatcher {
    let mut dispatcher = KernelDispatcher::new();
    syscalls::register_all(&mut dispatcher);
    dispatcher
}

fn make_frame_with_actor(name: &str, data: serde_json::Value, actor: &str) -> Frame {
    Frame::req(name, data).with_actor(actor)
}

#[tokio::test]
async fn test_fs_read_memory_file_not_found() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    init_mounts(&tmp);
    let dispatcher = setup_dispatcher();

    let req = Frame::req("fs:read", json!({ "path": "/test.txt" }));
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let response = rx.recv().await.expect("should receive response");
    assert_eq!(response.op, FrameOp::Error);
    let data = response.data.unwrap();
    assert_eq!(data["code"], "E_NOT_FOUND");
}

#[tokio::test]
async fn test_exec_run_requires_head_scope() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher();

    // mkdir is always-mutating, so no-actor dispatch should be rejected
    let req = Frame::req("exec:run", json!({ "program": "mkdir", "args": ["foo"] }));
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let response = rx.recv().await.expect("should receive response");
    assert_eq!(response.op, FrameOp::Error);
    let data = response.data.unwrap();
    assert_eq!(data["code"], "E_FORBIDDEN");
}

#[tokio::test]
async fn test_exec_run_with_head_scope() {
    let tmp = TempDir::new().unwrap();
    init_mounts(&tmp);
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher();

    let req = make_frame_with_actor(
        "exec:run",
        json!({ "program": "echo", "args": ["hello", "world"] }),
        "head/test",
    );
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let response = rx.recv().await.expect("should receive response");
    assert_eq!(response.op, FrameOp::Ok);

    let data = response.data.unwrap();
    assert!(data["success"].as_bool().unwrap());
    assert!(data["stdout"].as_str().unwrap().contains("hello world"));
}

#[tokio::test]
async fn test_exec_run_forbidden_program() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher();

    let req = make_frame_with_actor(
        "exec:run",
        json!({ "program": "nc", "args": ["-l", "1234"] }),
        "head/test",
    );
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let response = rx.recv().await.expect("should receive error");
    assert_eq!(response.op, FrameOp::Error);

    let data = response.data.unwrap();
    assert_eq!(data["code"], "E_FORBIDDEN");
}

#[tokio::test]
async fn test_exec_run_cancellation() {
    let tmp = TempDir::new().unwrap();
    init_mounts(&tmp);
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher();

    let cancel = CancellationToken::new();

    let req = make_frame_with_actor(
        "exec:run",
        json!({ "program": "sleep", "args": ["10"], "timeout_ms": 30000 }),
        "head/test",
    );

    let cancel_clone = cancel.clone();
    let mut rx = dispatcher.dispatch(req.clone(), workspace, cancel_clone);

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    cancel.cancel();

    let response = rx.recv().await.expect("should receive cancellation error");
    assert_eq!(response.op, FrameOp::Error);

    let data = response.data.unwrap();
    assert_eq!(data["code"], "E_CANCELLED");
}

#[tokio::test]
async fn test_unknown_syscall() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher();

    let req = Frame::req("unknown:syscall", json!({}));
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let response = rx.recv().await.expect("should receive error");
    assert_eq!(response.op, FrameOp::Error);

    let data = response.data.unwrap();
    assert_eq!(data["code"], "E_NOT_IMPLEMENTED");
}

#[tokio::test]
async fn test_frame_serialization_roundtrip() {
    let original = Frame::req("fs:read", json!({"path": "/test"}))
        .with_actor("hand/anonymous")
        .with_deadline(5000);

    let json = serde_json::to_string(&original).unwrap();
    let restored: Frame = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.id, original.id);
    assert_eq!(restored.op, original.op);
    assert_eq!(restored.name, original.name);
    assert_eq!(restored.actor, original.actor);
    assert_eq!(restored.deadline_ms, original.deadline_ms);
}

#[tokio::test]
async fn test_dispatcher_list_syscalls() {
    let dispatcher = setup_dispatcher();

    let syscalls = dispatcher.list();
    assert!(syscalls.contains(&"fs:read"));
    assert!(syscalls.contains(&"fs:write"));
    assert!(syscalls.contains(&"exec:run"));
    assert!(syscalls.contains(&"net:fetch"));
}
