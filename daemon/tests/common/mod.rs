use std::path::Path;
use std::sync::Arc;

use abbot::kernel::{Frame, FrameOp, KernelError, Syscall, SyscallContext};
use abbot::vfs::{MountConfig, MountMode, MountTable};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Create a MountTable backed by a temp dir path, mounted at "/" with rw access.
#[allow(dead_code)]
pub fn make_vfs(root: &Path) -> Arc<MountTable> {
    let config = MountConfig {
        prefix: "/".to_string(),
        host: root.to_string_lossy().to_string(),
        mode: MountMode::Rw,
    };
    Arc::new(MountTable::from_config(vec![config]).expect("failed to create mount table"))
}

/// Create a MountTable with a read-only mount.
#[allow(dead_code)]
pub fn make_vfs_readonly(root: &Path) -> Arc<MountTable> {
    let config = MountConfig {
        prefix: "/".to_string(),
        host: root.to_string_lossy().to_string(),
        mode: MountMode::Ro,
    };
    Arc::new(MountTable::from_config(vec![config]).expect("failed to create mount table"))
}

/// Create a SyscallContext with no actor (defaults to hand/anonymous).
#[allow(dead_code)]
pub fn make_ctx(cwd: &Path) -> SyscallContext {
    SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
}

/// Create a SyscallContext with the given actor string.
#[allow(dead_code)]
pub fn make_ctx_with_actor(cwd: &Path, actor: &str) -> SyscallContext {
    SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
        .with_actor(Some(actor.to_string()))
}

/// Execute a syscall and collect all response frames.
#[allow(dead_code)]
pub async fn exec(
    syscall: &dyn Syscall,
    ctx: &SyscallContext,
    data: Value,
) -> (Result<(), KernelError>, Vec<Frame>) {
    let (tx, mut rx) = mpsc::channel(64);
    let result = syscall.execute(ctx, data, tx).await;

    let mut frames = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        frames.push(frame);
    }

    (result, frames)
}

/// Assert that the syscall produced exactly one Ok frame and return its data.
#[allow(dead_code)]
pub fn assert_ok(frames: &[Frame]) -> &Value {
    let ok_frames: Vec<_> = frames
        .iter()
        .filter(|f| matches!(f.op, FrameOp::Ok))
        .collect();
    assert_eq!(
        ok_frames.len(),
        1,
        "expected exactly one Ok frame, got {}",
        ok_frames.len()
    );
    ok_frames[0]
        .data
        .as_ref()
        .expect("Ok frame should have data")
}

/// Assert that the syscall returned an error with the given code.
#[allow(dead_code)]
pub fn assert_error(result: &Result<(), KernelError>, code: &str) {
    let err = result
        .as_ref()
        .expect_err(&format!("expected error {code}, got Ok"));
    assert_eq!(
        err.code, code,
        "expected error code {code}, got {}",
        err.code
    );
}
