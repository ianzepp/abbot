use std::sync::Arc;

use abbot::hal::llm::UnifiedMessage as Message;
use abbot::history::Store;
use abbot::runtime::{
    HandBundleBuilder, HandBundleConfig, SnapshotManager, atomic_write_file_0600,
    workspace_head_memory,
};

/// Extract text content from a Message enum variant.
fn text(msg: &Message) -> &str {
    match msg {
        Message::System(s) => s,
        Message::User(s) => s,
        Message::Assistant(s) => s,
        _ => panic!("expected text message, got {:?}", msg),
    }
}

fn is_system(msg: &Message) -> bool {
    matches!(msg, Message::System(_))
}

fn is_user(msg: &Message) -> bool {
    matches!(msg, Message::User(_))
}

fn is_assistant(msg: &Message) -> bool {
    matches!(msg, Message::Assistant(_))
}

#[tokio::test]
async fn builds_initial_messages() {
    let store = Arc::new(Store::open(":memory:").await.unwrap());
    let builder = HandBundleBuilder::new(store, std::env::current_dir().unwrap()).await;

    let cfg = HandBundleConfig::new("t-1", "head-0", "list files", "");
    let messages = builder.build(&cfg).await;

    assert_eq!(messages.len(), 2);
    assert!(is_system(&messages[0]));
    assert!(text(&messages[0]).contains("You are a hand"));
    assert!(text(&messages[0]).contains("## Tools"));
    assert!(is_user(&messages[1]));
    assert!(text(&messages[1]).contains("list files"));
}

#[tokio::test]
async fn builds_conversation_from_history() {
    let store = Arc::new(Store::open(":memory:").await.unwrap());

    store
        .log_hand_exec(
            "t-2",
            "hand-1",
            0,
            "bash",
            "ls",
            "file1\nfile2",
            true,
            10,
            "<exec tool=\"bash\">ls</exec>",
        )
        .await
        .unwrap();
    store
        .log_hand_exec(
            "t-2",
            "hand-1",
            1,
            "read",
            "file1",
            "contents",
            true,
            5,
            "<exec tool=\"read\">file1</exec>",
        )
        .await
        .unwrap();

    let builder = HandBundleBuilder::new(store, std::env::current_dir().unwrap()).await;
    let cfg = HandBundleConfig::new("t-2", "head-1", "read files", "");
    let messages = builder.build(&cfg).await;

    assert_eq!(messages.len(), 6);

    assert!(is_system(&messages[0]));
    assert!(is_user(&messages[1]));
    assert!(text(&messages[1]).contains("read files"));

    assert!(is_assistant(&messages[2]));
    assert!(text(&messages[2]).contains("<exec tool=\"bash\">ls</exec>"));

    assert!(is_user(&messages[3]));
    assert!(text(&messages[3]).contains("[Tool bash completed]"));
    assert!(text(&messages[3]).contains("file1\nfile2"));

    assert!(is_assistant(&messages[4]));
    assert!(text(&messages[4]).contains("<exec tool=\"read\">file1</exec>"));

    assert!(is_user(&messages[5]));
    assert!(text(&messages[5]).contains("[Tool read completed]"));
    assert!(text(&messages[5]).contains("contents"));
}

#[tokio::test]
async fn includes_stm_in_initial_prompt() {
    let store = Arc::new(Store::open(":memory:").await.unwrap());

    let temp_dir = tempfile::tempdir().unwrap();
    let memory_path = workspace_head_memory(temp_dir.path(), "head-2");
    std::fs::create_dir_all(memory_path.parent().unwrap()).unwrap();
    atomic_write_file_0600(
        &memory_path,
        "Working on refactoring auth module.\nUser prefers functional style.",
    )
    .unwrap();

    let snapshot = SnapshotManager::new(temp_dir.path().to_path_buf(), Some(store.clone())).await;
    let builder =
        HandBundleBuilder::new_with_snapshot(store, temp_dir.path().to_path_buf(), snapshot);
    let cfg = HandBundleConfig::new("t-3", "head-2", "update login function", "");
    let messages = builder.build(&cfg).await;

    assert_eq!(messages.len(), 2);

    let initial = text(&messages[1]);
    assert!(initial.contains("CONTEXT"));
    assert!(initial.contains("refactoring auth module"));
    assert!(initial.contains("functional style"));
    assert!(initial.contains("update login function"));
}

#[tokio::test]
async fn skips_empty_stm() {
    let store = Arc::new(Store::open(":memory:").await.unwrap());

    let temp_dir = tempfile::tempdir().unwrap();
    let snapshot = SnapshotManager::new(temp_dir.path().to_path_buf(), Some(store.clone())).await;
    let builder =
        HandBundleBuilder::new_with_snapshot(store, temp_dir.path().to_path_buf(), snapshot);

    let cfg = HandBundleConfig::new("t-4", "head-3", "list files", "");
    let messages = builder.build(&cfg).await;

    let initial = text(&messages[1]);
    assert!(!initial.contains("CONTEXT"));
    assert!(initial.contains("list files"));
}
