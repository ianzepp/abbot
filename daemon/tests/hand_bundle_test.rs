use std::sync::Arc;

use abbot::history::Store;
use abbot::hal::llm::Role;
use abbot::runtime::{
    HandBundleBuilder, HandBundleConfig, SnapshotManager,
    atomic_write_file_0600, workspace_head_memory,
};

#[test]
fn builds_initial_messages() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    let builder = HandBundleBuilder::new(store, std::env::current_dir().unwrap());

    let cfg = HandBundleConfig::new("t-1", "head-0", "list files", "");
    let messages = builder.build(&cfg);

    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0].role, Role::System));
    assert!(
        messages[0]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("You are a hand")
    );
    assert!(
        messages[0]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("## Tools")
    );
    assert!(matches!(messages[1].role, Role::User));
    assert!(
        messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("goal: list files")
    );
}

#[test]
fn builds_conversation_from_history() {
    let store = Arc::new(Store::open(":memory:").unwrap());

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
        .unwrap();

    let builder = HandBundleBuilder::new(store, std::env::current_dir().unwrap());
    let cfg = HandBundleConfig::new("t-2", "head-1", "read files", "");
    let messages = builder.build(&cfg);

    assert_eq!(messages.len(), 6);

    assert!(matches!(messages[0].role, Role::System));
    assert!(matches!(messages[1].role, Role::User));
    assert!(
        messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("goal: read files")
    );

    assert!(matches!(messages[2].role, Role::Assistant));
    assert!(
        messages[2]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("<exec tool=\"bash\">ls</exec>")
    );

    assert!(matches!(messages[3].role, Role::User));
    assert!(
        messages[3]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("[Tool bash completed]")
    );
    assert!(
        messages[3]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("file1\nfile2")
    );

    assert!(matches!(messages[4].role, Role::Assistant));
    assert!(
        messages[4]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("<exec tool=\"read\">file1</exec>")
    );

    assert!(matches!(messages[5].role, Role::User));
    assert!(
        messages[5]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("[Tool read completed]")
    );
    assert!(
        messages[5]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("contents")
    );
}

#[test]
fn includes_stm_in_initial_prompt() {
    let store = Arc::new(Store::open(":memory:").unwrap());

    let temp_dir = tempfile::tempdir().unwrap();
    let memory_path = workspace_head_memory(temp_dir.path(), "head-2");
    std::fs::create_dir_all(memory_path.parent().unwrap()).unwrap();
    atomic_write_file_0600(
        &memory_path,
        "Working on refactoring auth module.\nUser prefers functional style.",
    )
    .unwrap();

    let snapshot = SnapshotManager::new(temp_dir.path().to_path_buf(), Some(store.clone()));
    let builder =
        HandBundleBuilder::new_with_snapshot(store, temp_dir.path().to_path_buf(), snapshot);
    let cfg = HandBundleConfig::new("t-3", "head-2", "update login function", "");
    let messages = builder.build(&cfg);

    assert_eq!(messages.len(), 2);

    let initial = messages[1].content.as_deref().unwrap_or("");
    assert!(initial.contains("CONTEXT"));
    assert!(initial.contains("refactoring auth module"));
    assert!(initial.contains("functional style"));
    assert!(initial.contains("goal: update login function"));
}

#[test]
fn skips_empty_stm() {
    let store = Arc::new(Store::open(":memory:").unwrap());

    let temp_dir = tempfile::tempdir().unwrap();
    let snapshot = SnapshotManager::new(temp_dir.path().to_path_buf(), Some(store.clone()));
    let builder =
        HandBundleBuilder::new_with_snapshot(store, temp_dir.path().to_path_buf(), snapshot);

    let cfg = HandBundleConfig::new("t-4", "head-3", "list files", "");
    let messages = builder.build(&cfg);

    let initial = messages[1].content.as_deref().unwrap_or("");
    assert!(!initial.contains("CONTEXT"));
    assert!(initial.contains("goal: list files"));
}
