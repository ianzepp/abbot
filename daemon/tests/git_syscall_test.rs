use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use abbot::kernel::Syscall;
use abbot::kernel::{FrameOp, SyscallContext};
use abbot::syscalls::git::GitRun;

fn make_ctx(cwd: &std::path::Path) -> SyscallContext {
    SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
}

fn make_ctx_with_actor(cwd: &std::path::Path, actor: &str) -> SyscallContext {
    SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
        .with_actor(Some(actor.to_string()))
}

#[tokio::test]
async fn test_git_status_readonly_allowed() {
    let tmp = TempDir::new().unwrap();

    std::process::Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .ok();

    let syscall = GitRun::new();
    let ctx = make_ctx(tmp.path());
    let (tx, mut rx) = mpsc::channel(8);

    let result = syscall
        .execute(&ctx, json!({ "args": ["status"] }), tx)
        .await;

    assert!(result.is_ok());
    let frame = rx.recv().await.unwrap();
    assert_eq!(frame.op, FrameOp::Ok);
    assert!(frame.data.unwrap()["success"].as_bool().unwrap());
}

#[tokio::test]
async fn test_git_log_readonly_allowed() {
    let tmp = TempDir::new().unwrap();

    std::process::Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .ok();

    let syscall = GitRun::new();
    let ctx = make_ctx(tmp.path());
    let (tx, mut rx) = mpsc::channel(8);

    let result = syscall
        .execute(&ctx, json!({ "args": ["log", "--oneline", "-n", "5"] }), tx)
        .await;

    assert!(result.is_ok());
    let frame = rx.recv().await.unwrap();
    assert_eq!(frame.op, FrameOp::Ok);
}

#[tokio::test]
async fn test_git_add_requires_mutation() {
    let tmp = TempDir::new().unwrap();

    std::process::Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .ok();

    let syscall = GitRun::new();
    let ctx = make_ctx(tmp.path());
    let (tx, _rx) = mpsc::channel(8);

    let result = syscall
        .execute(&ctx, json!({ "args": ["add", "."] }), tx)
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, "E_FORBIDDEN");
}

#[tokio::test]
async fn test_git_add_with_head_scope() {
    let tmp = TempDir::new().unwrap();

    std::process::Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .ok();

    let syscall = GitRun::new();
    let ctx = make_ctx_with_actor(tmp.path(), "head/test");
    let (tx, mut rx) = mpsc::channel(8);

    let result = syscall
        .execute(&ctx, json!({ "args": ["add", "."] }), tx)
        .await;

    assert!(result.is_ok());
    let frame = rx.recv().await.unwrap();
    assert_eq!(frame.op, FrameOp::Ok);
}

#[tokio::test]
async fn test_git_push_forbidden() {
    let tmp = TempDir::new().unwrap();
    let syscall = GitRun::new();
    let ctx = make_ctx_with_actor(tmp.path(), "head/test");
    let (tx, _rx) = mpsc::channel(8);

    let result = syscall
        .execute(&ctx, json!({ "args": ["push", "origin", "main"] }), tx)
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, "E_FORBIDDEN");
}

#[tokio::test]
async fn test_git_config_forbidden() {
    let tmp = TempDir::new().unwrap();
    let syscall = GitRun::new();
    let ctx = make_ctx_with_actor(tmp.path(), "head/test");
    let (tx, _rx) = mpsc::channel(8);

    let result = syscall
        .execute(
            &ctx,
            json!({ "args": ["config", "user.email", "test@example.com"] }),
            tx,
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, "E_FORBIDDEN");
}
