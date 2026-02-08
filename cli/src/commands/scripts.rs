//! Scripts - Offline diagnostic commands for inspecting agent bundles
//!
//! Each script initializes a minimal Kernel with read-only DB access,
//! runs the relevant bundle builder, and prints the resulting messages
//! to stdout. No running daemon required.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Subcommand;

use abbot::ems::EmsService;
use abbot::hal::llm::{ChatMessage, Role, UnifiedMessage};
use abbot::history::Store;
use abbot::kernel::FrameStore;
use abbot::runtime::Kernel;
use abbot::runtime::app_config::WorkspacePaths;
use abbot::runtime::{
    HandBundleBuilder, HandBundleConfig, HeadBundleBuilder, HeadBundleConfig,
    MindLoopBundleBuilder, MindLoopBundleConfig, RoomBundleBuilder, RoomBundleConfig, RoomType,
    WakeMode,
};
use abbot::scope::Scope;

use crate::config;
use crate::error::CliError;
use crate::output::OutputFormat;

// =============================================================================
// SUBCOMMANDS
// =============================================================================

#[derive(Subcommand)]
pub enum ScriptsAction {
    /// Show the mind loop bundle (system + user messages)
    Mind {
        /// Channel name
        #[arg(long, default_value = "#main")]
        channel: String,
    },
    /// Show the head agent bundle (system + conversation history)
    Head {
        /// Scope name
        #[arg(long, default_value = "#main")]
        scope: String,
        /// Head identity
        #[arg(long, default_value = "Monk")]
        head_id: String,
    },
    /// Show the room/mind meeting bundle (system + user message)
    Room {
        /// Scope name
        #[arg(long, default_value = "#main")]
        scope: String,
        /// Head identity
        #[arg(long, default_value = "Monk")]
        head_id: String,
        /// Room type (conclave, autonomy, work)
        #[arg(long, default_value = "conclave")]
        room_type: String,
    },
    /// Show the hand agent bundle (system + task context)
    Hand {
        /// Head identity
        #[arg(long, default_value = "Monk")]
        head_id: String,
        /// Task ID
        #[arg(long)]
        task_id: String,
        /// Task prompt
        #[arg(long)]
        prompt: String,
    },
}

// =============================================================================
// DISPATCHER
// =============================================================================

pub async fn run(
    cli_config: Option<PathBuf>,
    action: ScriptsAction,
    format: OutputFormat,
) -> Result<(), CliError> {
    let (store, home) = init_kernel_readonly(cli_config.as_deref()).await?;

    match action {
        ScriptsAction::Mind { channel } => run_mind_bundle(store, &home, &channel, format).await,
        ScriptsAction::Head { scope, head_id } => {
            run_head_bundle(store, &home, &scope, &head_id, format).await
        }
        ScriptsAction::Room {
            scope,
            head_id,
            room_type,
        } => run_room_bundle(store, &home, &scope, &head_id, &room_type, format).await,
        ScriptsAction::Hand {
            head_id,
            task_id,
            prompt,
        } => run_hand_bundle(store, &home, &head_id, &task_id, &prompt, format).await,
    }
}

// =============================================================================
// KERNEL BOOTSTRAP
// =============================================================================

async fn init_kernel_readonly(
    cli_config: Option<&Path>,
) -> Result<(Arc<Store>, PathBuf), CliError> {
    config::init_app_config(cli_config);

    let home = dirs::home_dir().ok_or(CliError::General("no home directory".into()))?;
    let paths = WorkspacePaths::new(home.clone());

    Kernel::init(&paths.home);

    let store = Arc::new(
        Store::open(&paths.store_db)
            .await
            .map_err(|e| CliError::General(format!("failed to open store.db: {e}")))?,
    );

    if let Some(k) = Kernel::get() {
        k.set_store(store.clone());
    }

    if paths.frames_db.exists()
        && let Ok(frames) = FrameStore::open(&paths.frames_db).await
        && let Some(k) = Kernel::get()
    {
        k.set_frames(frames).await;
    }

    if paths.ems_db.exists()
        && let Ok(ems) = EmsService::open(&paths.ems_db).await
        && let Some(k) = Kernel::get()
    {
        k.set_ems(ems.handle());
    }

    Ok((store, home))
}

// =============================================================================
// SCRIPT HANDLERS
// =============================================================================

async fn run_mind_bundle(
    store: Arc<Store>,
    home: &Path,
    channel: &str,
    format: OutputFormat,
) -> Result<(), CliError> {
    let cfg = MindLoopBundleConfig::new(channel, home.to_path_buf());
    let builder = MindLoopBundleBuilder::new(store);
    let messages = builder.build(&cfg).await;
    print_chat_messages(&messages, format);
    Ok(())
}

async fn run_head_bundle(
    store: Arc<Store>,
    home: &Path,
    scope: &str,
    head_id: &str,
    format: OutputFormat,
) -> Result<(), CliError> {
    let scopes = vec![Scope::new(scope)];
    let cfg = HeadBundleConfig::new(head_id, scopes);
    let builder = HeadBundleBuilder::new(store, home.to_path_buf()).await;
    let messages = builder.build(&cfg).await;
    print_chat_messages(&messages, format);
    Ok(())
}

async fn run_room_bundle(
    store: Arc<Store>,
    home: &Path,
    scope: &str,
    head_id: &str,
    room_type_str: &str,
    format: OutputFormat,
) -> Result<(), CliError> {
    let room_type = match room_type_str {
        "conclave" => RoomType::Conclave,
        "autonomy" => RoomType::Autonomy,
        "work" => RoomType::Work,
        other => {
            return Err(CliError::General(format!(
                "unknown room type: {other} (expected conclave, autonomy, work)"
            )));
        }
    };

    let scopes = vec![Scope::new(scope)];
    let cfg = RoomBundleConfig::new(head_id, scopes)
        .with_wake_mode(WakeMode::Normal)
        .with_workspace(home.to_path_buf())
        .with_room_type(room_type);
    let builder = RoomBundleBuilder::new(store);
    let messages = builder.build(&cfg).await;
    print_chat_messages(&messages, format);
    Ok(())
}

async fn run_hand_bundle(
    store: Arc<Store>,
    home: &Path,
    head_id: &str,
    task_id: &str,
    prompt: &str,
    format: OutputFormat,
) -> Result<(), CliError> {
    let cfg = HandBundleConfig::new(task_id, head_id, prompt, "");
    let builder = HandBundleBuilder::new(store, home.to_path_buf()).await;
    let messages = builder.build(&cfg).await;
    print_unified_messages(&messages, format);
    Ok(())
}

// =============================================================================
// OUTPUT
// =============================================================================

fn print_chat_messages(messages: &[ChatMessage], format: OutputFormat) {
    match format.resolve() {
        OutputFormat::Json => {
            let arr: Vec<serde_json::Value> = messages
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "role": m.role,
                        "content": m.content,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&arr).unwrap_or_default());
        }
        _ => {
            for (i, m) in messages.iter().enumerate() {
                let role = match m.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "tool",
                };
                println!("=== Message {} ({}) ===", i + 1, role);
                if let Some(ref content) = m.content {
                    println!("{content}");
                }
                println!();
            }
        }
    }
}

fn print_unified_messages(messages: &[UnifiedMessage], format: OutputFormat) {
    match format.resolve() {
        OutputFormat::Json => {
            let arr: Vec<serde_json::Value> = messages
                .iter()
                .map(|m| match m {
                    UnifiedMessage::System(s) => serde_json::json!({"role": "system", "content": s}),
                    UnifiedMessage::User(s) => serde_json::json!({"role": "user", "content": s}),
                    UnifiedMessage::Assistant(s) => {
                        serde_json::json!({"role": "assistant", "content": s})
                    }
                    UnifiedMessage::AssistantToolCalls(calls) => {
                        serde_json::json!({"role": "assistant", "tool_calls": calls.iter().map(|c| {
                            serde_json::json!({"id": c.id, "name": c.name, "arguments": c.arguments})
                        }).collect::<Vec<_>>()})
                    }
                    UnifiedMessage::ToolResult { id, content, is_error } => {
                        serde_json::json!({"role": "tool", "tool_call_id": id, "content": content, "is_error": is_error})
                    }
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&arr).unwrap_or_default());
        }
        _ => {
            for (i, m) in messages.iter().enumerate() {
                let (role, content) = match m {
                    UnifiedMessage::System(s) => ("system", s.as_str()),
                    UnifiedMessage::User(s) => ("user", s.as_str()),
                    UnifiedMessage::Assistant(s) => ("assistant", s.as_str()),
                    UnifiedMessage::AssistantToolCalls(_) => ("assistant", "[tool calls]"),
                    UnifiedMessage::ToolResult { content, .. } => ("tool", content.as_str()),
                };
                println!("=== Message {} ({}) ===", i + 1, role);
                println!("{content}");
                println!();
            }
        }
    }
}
