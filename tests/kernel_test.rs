use std::path::PathBuf;

use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use abbot::kernel::{Frame, FrameOp, KernelDispatcher};
use abbot::syscalls;

fn setup_dispatcher(workspace: PathBuf) -> KernelDispatcher {
    let mut dispatcher = KernelDispatcher::new(workspace);
    syscalls::register_all(&mut dispatcher);
    dispatcher
}

#[tokio::test]
async fn test_fs_read_cargo_toml() {
    let workspace = std::env::current_dir().unwrap();
    let dispatcher = setup_dispatcher(workspace.clone());

    let req = Frame::req("fs:read", json!({ "path": "Cargo.toml", "limit": 10 }));
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let response = rx.recv().await.expect("should receive response");
    assert_eq!(response.op, FrameOp::Ok);
    assert_eq!(response.parent_id, Some(req.id));

    let data = response.data.unwrap();
    let content = data["content"].as_str().unwrap();
    assert!(content.contains("[package]"));
    assert!(content.contains("abbot"));
}

#[tokio::test]
async fn test_fs_read_not_found() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher(workspace.clone());

    let req = Frame::req("fs:read", json!({ "path": "nonexistent.txt" }));
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let response = rx.recv().await.expect("should receive error");
    assert_eq!(response.op, FrameOp::Error);
    assert_eq!(response.parent_id, Some(req.id));

    let data = response.data.unwrap();
    assert_eq!(data["code"], "E_NOT_FOUND");
}

#[tokio::test]
async fn test_fs_write_and_read() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher(workspace.clone());

    let write_req = Frame::req(
        "fs:write",
        json!({ "path": "test_output.txt", "content": "hello kernel" }),
    );
    let mut rx = dispatcher.dispatch(write_req.clone(), workspace.clone(), CancellationToken::new());

    let write_response = rx.recv().await.expect("should receive write response");
    assert_eq!(write_response.op, FrameOp::Ok);

    let read_req = Frame::req("fs:read", json!({ "path": "test_output.txt" }));
    let mut rx = dispatcher.dispatch(read_req.clone(), workspace.clone(), CancellationToken::new());

    let read_response = rx.recv().await.expect("should receive read response");
    assert_eq!(read_response.op, FrameOp::Ok);
    assert_eq!(
        read_response.data.unwrap()["content"].as_str().unwrap(),
        "hello kernel"
    );
}

#[tokio::test]
async fn test_proc_run_git_status() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    std::process::Command::new("git")
        .args(["init"])
        .current_dir(&workspace)
        .output()
        .expect("git init should work");

    let dispatcher = setup_dispatcher(workspace.clone());

    let req = Frame::req("proc:run", json!({ "program": "git", "args": ["status"] }));
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let response = rx.recv().await.expect("should receive response");
    assert_eq!(response.op, FrameOp::Ok);

    let data = response.data.unwrap();
    assert!(data["success"].as_bool().unwrap());
    assert!(data["stdout"].as_str().unwrap().contains("branch"));
}

#[tokio::test]
async fn test_proc_run_forbidden_program() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher(workspace.clone());

    let req = Frame::req("proc:run", json!({ "program": "nc", "args": ["-l", "1234"] }));
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let response = rx.recv().await.expect("should receive error");
    assert_eq!(response.op, FrameOp::Error);

    let data = response.data.unwrap();
    assert_eq!(data["code"], "E_FORBIDDEN");
}

#[tokio::test]
async fn test_proc_run_cancellation() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher(workspace.clone());

    let cancel = CancellationToken::new();

    let req = Frame::req(
        "proc:run",
        json!({ "program": "sleep", "args": ["10"], "timeout_ms": 30000 }),
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
async fn test_workspace_escape_rejected() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher(workspace.clone());

    let req = Frame::req("fs:read", json!({ "path": "/etc/passwd" }));
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let response = rx.recv().await.expect("should receive error");
    assert_eq!(response.op, FrameOp::Error);

    let data = response.data.unwrap();
    assert_eq!(data["code"], "E_FORBIDDEN");
}

#[tokio::test]
async fn test_git_run_status() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    std::process::Command::new("git")
        .args(["init"])
        .current_dir(&workspace)
        .output()
        .expect("git init should work");

    let dispatcher = setup_dispatcher(workspace.clone());

    let req = Frame::req("git:run", json!({ "args": ["status", "--short"] }));
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let response = rx.recv().await.expect("should receive response");
    assert_eq!(response.op, FrameOp::Ok);

    let data = response.data.unwrap();
    assert!(data["success"].as_bool().unwrap());
}

#[tokio::test]
async fn test_git_push_forbidden() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher(workspace.clone());

    let req = Frame::req("git:run", json!({ "args": ["push", "origin", "main"] }));
    let mut rx = dispatcher.dispatch(req.clone(), workspace, CancellationToken::new());

    let response = rx.recv().await.expect("should receive error");
    assert_eq!(response.op, FrameOp::Error);

    let data = response.data.unwrap();
    assert_eq!(data["code"], "E_FORBIDDEN");
}

#[tokio::test]
async fn test_unknown_syscall() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher(workspace.clone());

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
        .with_scope("tasks/abc123")
        .with_deadline(5000);

    let json = serde_json::to_string(&original).unwrap();
    let restored: Frame = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.id, original.id);
    assert_eq!(restored.op, original.op);
    assert_eq!(restored.name, original.name);
    assert_eq!(restored.scope, original.scope);
    assert_eq!(restored.deadline_ms, original.deadline_ms);
}

#[tokio::test]
async fn test_dispatcher_list_syscalls() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let dispatcher = setup_dispatcher(workspace);

    let syscalls = dispatcher.list();
    assert!(syscalls.contains(&"fs:read"));
    assert!(syscalls.contains(&"fs:write"));
    assert!(syscalls.contains(&"proc:run"));
    assert!(syscalls.contains(&"net:fetch"));
    assert!(syscalls.contains(&"git:run"));
}
