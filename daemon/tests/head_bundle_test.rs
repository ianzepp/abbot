use std::sync::Arc;

use abbot::history::Store;
use abbot::kernel::{FrameStore, Frame};
use abbot::hal::llm::Role;
use abbot::runtime::{HeadBundleBuilder, HeadBundleConfig, Kernel};
use abbot::scope::Scope;
use uuid::Uuid;

async fn ensure_kernel_with_frames() -> Arc<Kernel> {
    if let Some(k) = Kernel::get() {
        if k.frames().is_some() {
            return k;
        }
    }

    let root = std::env::temp_dir().join(format!("abbot-head-bundle-test-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let k = Kernel::get().unwrap_or_else(|| Kernel::init(&root));
    if k.frames().is_none() {
        let frames_db = root.join("frames.db");
        let store = FrameStore::open(&frames_db).unwrap();
        k.set_frames(store).await;
    }
    k
}

async fn dispatch(req: Frame) {
    let k = ensure_kernel_with_frames().await;
    let dispatcher = k.dispatcher().await;
    let mut rx = dispatcher.dispatch(
        req,
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        tokio_util::sync::CancellationToken::new(),
    );
    let _ = rx.recv().await;
}

#[tokio::test]
async fn builds_conversation_with_roles() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    let _ = ensure_kernel_with_frames().await;

    dispatch(
        Frame::req(
            "frames:append",
            serde_json::json!({
                "kind": "chat:user",
                "scope": "#general",
                "data": {"content": "hello monk"}
            }),
        )
        .with_actor("human/alice"),
    )
    .await;

    dispatch(
        Frame::req(
            "frames:append",
            serde_json::json!({
                "kind": "chat:head",
                "scope": "#general",
                "data": {"sender": "Monk", "content": "hello alice"}
            }),
        )
        .with_actor("head/Monk"),
    )
    .await;

    dispatch(
        Frame::req(
            "frames:append",
            serde_json::json!({
                "kind": "chat:user",
                "scope": "#general",
                "data": {"content": "can you help?"}
            }),
        )
        .with_actor("human/alice"),
    )
    .await;

    let builder = HeadBundleBuilder::new(store, std::env::current_dir().unwrap());
    let cfg = HeadBundleConfig::new("Monk", vec![Scope::from("#general")]);
    let messages = builder.build(&cfg);

    assert!(matches!(messages[0].role, Role::System));
    assert!(
        messages[0]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("Head")
    );

    let conversation: Vec<_> = messages
        .iter()
        .filter(|m| !matches!(m.role, Role::System))
        .collect();

    assert!(
        conversation.iter().any(|m| {
            matches!(m.role, Role::User)
                && m.content.as_deref().unwrap_or("").contains("hello monk")
                && m.content.as_deref().unwrap_or("").contains("alice")
        }),
        "expected hello monk message"
    );

    assert!(
        conversation.iter().any(|m| {
            matches!(m.role, Role::Assistant)
                && m.content.as_deref().unwrap_or("").contains("hello alice")
        }),
        "expected assistant reply"
    );

    assert!(
        conversation.iter().any(|m| {
            matches!(m.role, Role::User)
                && m.content.as_deref().unwrap_or("").contains("can you help")
        }),
        "expected follow-up question"
    );
}

#[tokio::test]
async fn includes_task_messages() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    let _ = ensure_kernel_with_frames().await;

    let scope = format!("#test-{}", Uuid::new_v4());

    dispatch(
        Frame::req(
            "task:enqueue",
            serde_json::json!({
                "task_id": "t-1",
                "head_id": "Monk",
                "goal": "do the thing",
                "input": "",
                "scope": scope,
                "notify_scope": scope
            }),
        )
        .with_actor("head/Monk"),
    )
    .await;

    dispatch(
        Frame::req(
            "task:complete",
            serde_json::json!({
                "task_id": "t-1",
                "ok": true,
                "summary": "done"
            }),
        )
        .with_actor("hand/hand-1"),
    )
    .await;

    let builder = HeadBundleBuilder::new(store, std::env::current_dir().unwrap());
    let cfg = HeadBundleConfig::new("Monk", vec![Scope::from(scope.as_str())]);
    let messages = builder.build(&cfg);

    let has_task = messages
        .iter()
        .skip_while(|m| matches!(m.role, Role::System))
        .any(|m| {
            matches!(m.role, Role::User)
                && m.content.as_deref().unwrap_or("").contains("task t-1")
        });
    assert!(has_task, "expected task t-1 summary to appear");
}
