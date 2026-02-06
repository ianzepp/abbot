mod common;

use abbot::syscalls::text::TextEcho;
use serde_json::json;
use tempfile::TempDir;

use common::{assert_error, assert_ok, exec, make_ctx};

#[tokio::test]
async fn test_text_echo_returns_input() {
    let tmp = TempDir::new().unwrap();
    let syscall = TextEcho::new();
    let ctx = make_ctx(tmp.path());

    let (result, frames) = exec(&syscall, &ctx, json!({"text": "hello world"})).await;
    assert!(result.is_ok());

    let data = assert_ok(&frames);
    assert_eq!(data["text"].as_str().unwrap(), "hello world");
}

#[tokio::test]
async fn test_text_echo_missing_text_field() {
    let tmp = TempDir::new().unwrap();
    let syscall = TextEcho::new();
    let ctx = make_ctx(tmp.path());

    let (result, _frames) = exec(&syscall, &ctx, json!({})).await;
    assert_error(&result, "E_INVALID_ARGS");
}
