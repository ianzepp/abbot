// Re-enabled: VFS sandbox root mount allows test helpers to mount at "/" again.

mod common;

use std::sync::Arc;

use abbot::hal::HostHalFs;
use abbot::syscalls::{FsGrep, FsList, FsMkdir, FsRead, FsWrite};
use serde_json::json;
use tempfile::TempDir;

use common::{
    assert_error, assert_ok, exec, make_ctx, make_ctx_with_actor, make_vfs, make_vfs_readonly,
};

// =============================================================================
// fs:read
// =============================================================================

#[tokio::test]
async fn test_fs_read_happy_path() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), "hello world\n").unwrap();

    let vfs = make_vfs(tmp.path());
    let syscall = FsRead::with_vfs(Arc::new(HostHalFs), vfs);
    let ctx = make_ctx(tmp.path());

    let (result, frames) = exec(&syscall, &ctx, json!({"path": "/hello.txt"})).await;
    assert!(result.is_ok());

    let data = assert_ok(&frames);
    let content = data["content"].as_str().unwrap();
    assert_eq!(content, "hello world\n");
}

#[tokio::test]
async fn test_fs_read_with_line_slicing() {
    let tmp = TempDir::new().unwrap();
    let content = "line1\nline2\nline3\nline4\nline5\n";
    std::fs::write(tmp.path().join("lines.txt"), content).unwrap();

    let vfs = make_vfs(tmp.path());
    let syscall = FsRead::with_vfs(Arc::new(HostHalFs), vfs);
    let ctx = make_ctx(tmp.path());

    let (result, frames) = exec(
        &syscall,
        &ctx,
        json!({"path": "/lines.txt", "offset": 1, "limit": 2}),
    )
    .await;
    assert!(result.is_ok());

    let data = assert_ok(&frames);
    let text = data["content"].as_str().unwrap();
    assert!(text.contains("line2"));
    assert!(text.contains("line3"));
    assert!(!text.contains("line1"));
    assert!(!text.contains("line4"));
}

#[tokio::test]
async fn test_fs_read_file_not_found() {
    let tmp = TempDir::new().unwrap();
    let vfs = make_vfs(tmp.path());
    let syscall = FsRead::with_vfs(Arc::new(HostHalFs), vfs);
    let ctx = make_ctx(tmp.path());

    let (result, _frames) = exec(&syscall, &ctx, json!({"path": "/nonexistent.txt"})).await;
    assert_error(&result, "E_NOT_FOUND");
}

#[tokio::test]
async fn test_fs_read_empty_path() {
    let tmp = TempDir::new().unwrap();
    let vfs = make_vfs(tmp.path());
    let syscall = FsRead::with_vfs(Arc::new(HostHalFs), vfs);
    let ctx = make_ctx(tmp.path());

    let (result, _frames) = exec(&syscall, &ctx, json!({"path": ""})).await;
    assert_error(&result, "E_INVALID_ARGS");
}

// =============================================================================
// fs:write
// =============================================================================

#[tokio::test]
async fn test_fs_write_happy_path() {
    let tmp = TempDir::new().unwrap();
    let vfs = make_vfs(tmp.path());
    let syscall = FsWrite::with_vfs(Arc::new(HostHalFs), vfs);
    let ctx = make_ctx_with_actor(tmp.path(), "head/test");

    let (result, frames) = exec(
        &syscall,
        &ctx,
        json!({"path": "/output.txt", "content": "hello world"}),
    )
    .await;
    assert!(result.is_ok());

    let data = assert_ok(&frames);
    assert_eq!(data["bytes_written"], 11);

    let on_disk = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
    assert_eq!(on_disk, "hello world");
}

#[tokio::test]
async fn test_fs_write_requires_head_actor() {
    let tmp = TempDir::new().unwrap();
    let vfs = make_vfs(tmp.path());
    let syscall = FsWrite::with_vfs(Arc::new(HostHalFs), vfs);
    let ctx = make_ctx_with_actor(tmp.path(), "hand/worker");

    let (result, _frames) = exec(
        &syscall,
        &ctx,
        json!({"path": "/output.txt", "content": "sneaky"}),
    )
    .await;
    assert_error(&result, "E_FORBIDDEN");
}

#[tokio::test]
async fn test_fs_write_readonly_mount_rejected() {
    let tmp = TempDir::new().unwrap();
    let vfs = make_vfs_readonly(tmp.path());
    let syscall = FsWrite::with_vfs(Arc::new(HostHalFs), vfs);
    let ctx = make_ctx_with_actor(tmp.path(), "head/test");

    let (result, _frames) = exec(
        &syscall,
        &ctx,
        json!({"path": "/project/output.txt", "content": "blocked"}),
    )
    .await;
    assert_error(&result, "E_READONLY");
}

#[tokio::test]
async fn test_fs_write_with_create_dirs() {
    let tmp = TempDir::new().unwrap();
    let vfs = make_vfs(tmp.path());
    let syscall = FsWrite::with_vfs(Arc::new(HostHalFs), vfs);
    let ctx = make_ctx_with_actor(tmp.path(), "head/test");

    let (result, frames) = exec(
        &syscall,
        &ctx,
        json!({"path": "/deep/nested/file.txt", "content": "created", "create_dirs": true}),
    )
    .await;
    assert!(result.is_ok());
    assert_ok(&frames);

    let on_disk = std::fs::read_to_string(tmp.path().join("deep/nested/file.txt")).unwrap();
    assert_eq!(on_disk, "created");
}

#[tokio::test]
async fn test_fs_write_missing_parent_no_create() {
    let tmp = TempDir::new().unwrap();
    let vfs = make_vfs(tmp.path());
    let syscall = FsWrite::with_vfs(Arc::new(HostHalFs), vfs);
    let ctx = make_ctx_with_actor(tmp.path(), "head/test");

    let (result, _frames) = exec(
        &syscall,
        &ctx,
        json!({"path": "/missing/parent/file.txt", "content": "fail", "create_dirs": false}),
    )
    .await;
    assert_error(&result, "E_NOT_FOUND");
}

// =============================================================================
// fs:list
// =============================================================================

#[tokio::test]
async fn test_fs_list_directory() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
    std::fs::write(tmp.path().join("b.rs"), "b").unwrap();
    std::fs::create_dir(tmp.path().join("subdir")).unwrap();

    let vfs = make_vfs(tmp.path());
    let syscall = FsList::with_vfs(vfs);
    let ctx = make_ctx(tmp.path());

    let (result, frames) = exec(&syscall, &ctx, json!({"path": "/"})).await;
    assert!(result.is_ok());

    let data = assert_ok(&frames);
    let matches = data["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 3);

    let names: Vec<&str> = matches.iter().map(|m| m.as_str().unwrap()).collect();
    assert!(names.contains(&"a.txt"));
    assert!(names.contains(&"b.rs"));
    assert!(names.contains(&"subdir"));
}

#[tokio::test]
async fn test_fs_list_with_glob() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
    std::fs::write(tmp.path().join("b.rs"), "b").unwrap();
    std::fs::write(tmp.path().join("c.txt"), "c").unwrap();

    let vfs = make_vfs(tmp.path());
    let syscall = FsList::with_vfs(vfs);
    let ctx = make_ctx(tmp.path());

    let (result, frames) = exec(&syscall, &ctx, json!({"path": "/", "pattern": "*.txt"})).await;
    assert!(result.is_ok());

    let data = assert_ok(&frames);
    let matches = data["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 2);

    let names: Vec<&str> = matches.iter().map(|m| m.as_str().unwrap()).collect();
    assert!(names.contains(&"a.txt"));
    assert!(names.contains(&"c.txt"));
    assert!(!names.iter().any(|n| n.ends_with(".rs")));
}

// =============================================================================
// fs:mkdir
// =============================================================================

#[tokio::test]
async fn test_fs_mkdir_happy_path() {
    let tmp = TempDir::new().unwrap();
    let vfs = make_vfs(tmp.path());
    let syscall = FsMkdir::with_vfs(vfs);
    let ctx = make_ctx_with_actor(tmp.path(), "head/test");

    let (result, frames) = exec(&syscall, &ctx, json!({"path": "/newdir"})).await;
    assert!(result.is_ok());

    let data = assert_ok(&frames);
    assert_eq!(data["created"], true);
    assert!(tmp.path().join("newdir").is_dir());
}

#[tokio::test]
async fn test_fs_mkdir_requires_head_actor() {
    let tmp = TempDir::new().unwrap();
    let vfs = make_vfs(tmp.path());
    let syscall = FsMkdir::with_vfs(vfs);
    let ctx = make_ctx_with_actor(tmp.path(), "hand/worker");

    let (result, _frames) = exec(&syscall, &ctx, json!({"path": "/blocked"})).await;
    assert_error(&result, "E_FORBIDDEN");
    assert!(!tmp.path().join("blocked").exists());
}

// =============================================================================
// fs:grep
// =============================================================================

#[tokio::test]
async fn test_fs_grep_literal() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("foo.txt"), "hello world\ngoodbye world\n").unwrap();
    std::fs::write(tmp.path().join("bar.txt"), "no match here\n").unwrap();

    let vfs = make_vfs(tmp.path());
    let syscall = FsGrep::with_vfs(vfs);
    let ctx = make_ctx(tmp.path());

    let (result, frames) = exec(&syscall, &ctx, json!({"query": "hello", "path": "/"})).await;
    assert!(result.is_ok());

    let data = assert_ok(&frames);
    let matches = data["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["text"].as_str().unwrap(), "hello world");
    assert_eq!(matches[0]["line"], 1);
}
