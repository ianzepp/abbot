//! Abbot - Persistent AI Background Daemon
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Abbot is a workspace-scoped AI daemon built on a syscall-driven kernel.
//! The kernel orchestrates three agent types (heads, hands, minds) via a
//! structured syscall interface (chat:*, llm:*, need:*, task:*).
//!
//! WHY a daemon model: Long-running context enables persistent memory, background
//! reflection, and proactive task execution without per-request initialization cost.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Workspace-scoped: Each abbot instance is tied to a working directory
//! - Agent specialization: Heads (decide), Hands (execute), Minds (reflect)
//! - Syscall-driven: All cross-agent communication flows through kernel syscalls
//! - Protocol adapters: OpenAI-compatible HTTP, web chat SSE, and future protocols
//!   are thin adapters over the same internal turn pipeline
//!
//! TIMING MODEL
//! ============
//! - Tick: 60 seconds (fixed)
//! - Default sleep: 300 seconds (5 ticks)
//! - Wake debounce: 5 seconds
//!
//! NOTE: Administrative commands (info, service, reset, providers, plugin, memory,
//! frames, monitor, tui) have moved to the `abbot` CLI binary.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;

use abbot::Scope;
use abbot::history::Store;
use abbot::runtime::{
    AppConfig, HandService, HeadConfig, HeadService, Kernel, MindLoop, RoomCoordinator,
    SessionWriteLocks,
};
use abbot::server::Server;

const DEFAULT_HEAD_ID: &str = "Abbot";

// =============================================================================
// CLI STRUCTURE AND ARGUMENT PARSING
// =============================================================================

#[derive(Parser, Clone)]
#[command(name = "abbotd")]
#[command(about = "Abbot: persistent AI background daemon", version)]
struct Cli {
    /// Path to config file (default: ~/.abbot/abbot.toml)
    #[arg(long)]
    config: Option<PathBuf>,

    /// API server address (host:port)
    #[arg(long)]
    addr: Option<String>,

    /// Exit after processing (use with `run prompt` for testing)
    #[arg(long)]
    exit: bool,

    /// Proxy mode: forward OpenAI-compatible requests to an upstream backend unchanged
    #[arg(long)]
    proxy: bool,

    /// Convene a conclave on boot (first-boot init or regular boot)
    #[arg(long)]
    conclave: bool,

    /// Log output format: default, compact, pretty
    #[arg(long)]
    log_format: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Clone)]
enum Command {
    /// Run the daemon (default), optionally with a frontend
    Run {
        #[command(subcommand)]
        frontend: Option<RunFrontend>,
    },
}

#[derive(clap::Subcommand, Clone)]
enum RunFrontend {
    /// Run with opencode TUI frontend
    Opencode {
        /// Additional arguments to pass to opencode
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Run the daemon and enqueue a prompt as a need
    Prompt {
        /// Prompt to enqueue
        prompt: String,
    },
    /// Run with claude CLI frontend
    Claude {
        /// Additional arguments to pass to claude
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Run with web UI (opens browser)
    Web,
}

// =============================================================================
// MAIN ENTRY POINT
// =============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command.clone() {
        None | Some(Command::Run { frontend: None }) => run_daemon(cli, None, None).await,
        Some(Command::Run {
            frontend: Some(RunFrontend::Prompt { prompt }),
        }) => run_daemon(cli, None, Some(prompt)).await,
        Some(Command::Run { frontend: Some(f) }) => run_daemon(cli, Some(f), None).await,
    }
}

// =============================================================================
// CONFIG HELPERS
// =============================================================================

/// Load API keys from ~/.abbot/keys.env and set as environment variables.
fn load_api_keys() -> Vec<(String, String)> {
    let path = match abbot::runtime::app_config::keys_path() {
        Some(p) => p,
        None => return Vec::new(),
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut loaded = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !key.is_empty() && !value.is_empty() {
                unsafe {
                    std::env::set_var(key, value);
                }
                let masked = if value.len() > 8 {
                    format!("{}...{}", &value[..4], &value[value.len() - 4..])
                } else {
                    "****".to_string()
                };
                loaded.push((key.to_string(), masked));
            }
        }
    }
    loaded
}

fn update_opencode_config(addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let base_url = format!("http://{}/v1", addr);

    let config_dir = dirs::home_dir()
        .ok_or("could not find home directory")?
        .join(".config")
        .join("opencode");

    std::fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join("opencode.json");

    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if config.get("provider").is_none() {
        config["provider"] = serde_json::json!({});
    }

    config["provider"]["abbot"] = serde_json::json!({
        "name": "Abbot",
        "npm": "@ai-sdk/openai-compatible",
        "options": {
            "baseURL": base_url,
            "apiKey": "not-required"
        },
        "models": {
            "abbot/default": {
                "name": "Abbot Default",
                "_launch": true
            }
        }
    });

    let content = serde_json::to_string_pretty(&config)?;
    std::fs::write(&config_path, content)?;

    Ok(())
}

// =============================================================================
// DAEMON RUNTIME
// =============================================================================

async fn run_daemon(
    cli: Cli,
    frontend: Option<RunFrontend>,
    initial_prompt: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::ems::EmsService;
    use abbot::runtime::app_config::{WorkspacePaths, default_config_path};

    // Require config to exist (user must run `abbot init` first)
    if cli.config.is_none() {
        let config_path = default_config_path().ok_or("could not determine config path")?;
        if !config_path.exists() {
            eprintln!("No configuration found at {}", config_path.display());
            eprintln!("Run `abbot init` to set up Abbot.");
            std::process::exit(1);
        }
    }

    // Load API keys from ~/.abbot/keys.env
    let loaded_keys = load_api_keys();
    if !loaded_keys.is_empty() {
        for (key, masked) in &loaded_keys {
            eprintln!("  loaded {} = {}", key, masked);
        }
    }

    // Initialize config first (before logging setup so we can get workspace path for logs)
    if let Some(ref path) = cli.config {
        AppConfig::init(path);
    } else if let Some(path) = default_config_path() {
        AppConfig::init(&path);
    } else {
        AppConfig::init_default();
    }

    // Construct workspace paths from home directory
    let home = dirs::home_dir().ok_or("could not determine home directory")?;
    let paths = WorkspacePaths::new(home);

    // Auto-create ~/.abbot/ directory if it doesn't exist
    if !paths.workspace.exists() {
        std::fs::create_dir_all(&paths.workspace)?;
        eprintln!("Created data directory: {}", paths.workspace.display());
    }

    // Resolve server bind addr + logging format now that config is loaded.
    let bind_addr = cli
        .addr
        .clone()
        .or_else(|| AppConfig::global().server.addr.clone())
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    abbot::runtime::set_effective_bind_addr(bind_addr.clone());

    let log_format = cli
        .log_format
        .clone()
        .or_else(|| AppConfig::global().server.log_format.clone())
        .unwrap_or_else(|| "default".to_string());

    // When running with a TUI frontend, redirect logs to a file to avoid corrupting the display
    let is_tui = matches!(
        frontend,
        Some(RunFrontend::Opencode { .. }) | Some(RunFrontend::Claude { .. })
    );
    if is_tui {
        let log_path = paths.workspace.join("daemon.log");
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        init_logging(&log_format, Some(log_file), false);
    } else {
        init_logging(&log_format, None, true);
    }

    tracing::debug!(config = ?AppConfig::global(), "app config loaded");

    // Create ~/.abbot/mind/ if missing
    if !paths.mind.exists() {
        std::fs::create_dir_all(&paths.mind)?;
        tracing::info!(path = %paths.mind.display(), "created mind directory");
    }

    // Create ~/.abbot/sandbox/ for persistent VFS root
    if !paths.sandbox.exists() {
        std::fs::create_dir_all(&paths.sandbox)?;
        tracing::info!(path = %paths.sandbox.display(), "created sandbox directory");
    }

    // Preflight checks (skip in proxy mode)
    if !cli.proxy
        && let Err(e) = abbot::runtime::run_preflight(&paths).await
    {
        tracing::error!(error = %e, "preflight failed — aborting");
        eprintln!("\nPreflight failed: {}", e);
        std::process::exit(1);
    }

    // Resolve database paths.
    let db_path = paths.store_db.clone();
    let ems_db_path = paths.ems_db.clone();
    let frames_db_path = paths.frames_db.clone();

    tracing::info!(
        home = %paths.home.display(),
        data = %paths.workspace.display(),
        "abbot starting"
    );

    if cli.proxy {
        tracing::info!(addr = %bind_addr, "starting in proxy mode");

        if initial_prompt.is_some() || cli.exit {
            tracing::warn!("prompt/--exit are ignored in --proxy mode");
        }
        if frontend.is_some() {
            tracing::warn!("frontend launch is ignored in --proxy mode");
        }

        let store = Arc::new(Store::open(":memory:").await?);
        Server::new(store, DEFAULT_HEAD_ID)
            .with_addr(&bind_addr)
            .with_proxy(true)
            .spawn();

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("shutdown requested");
                    break;
                }
                _ = std::future::pending::<()>() => {}
            }
        }

        return Ok(());
    }

    // Set working directory to ~ (the VFS root)
    std::env::set_current_dir(&paths.home)?;

    // Initialize kernel syscall dispatcher with VFS auto-mount
    Kernel::init(&paths.home);

    // Local TUI frame stream over Unix domain socket.
    #[cfg(unix)]
    {
        let sock = paths.frames_sock.clone();
        tokio::spawn(async move {
            if let Err(e) = abbot::runtime::serve_frames_uds(sock).await {
                tracing::warn!(error = %e, "frames UDS server failed");
            }
        });
    }

    let store = Arc::new(Store::open(&db_path).await?);
    tracing::debug!(db = %db_path.display(), "database opened");

    if let Some(k) = Kernel::get() {
        k.set_store(store.clone());
    }

    // Kernel frame store.
    match abbot::kernel::FrameStore::open(&frames_db_path).await {
        Ok(store) => {
            if let Some(k) = Kernel::get() {
                k.set_frames(store).await;
            }
            tracing::debug!(db = %frames_db_path.display(), "frames database opened");
        }
        Err(e) => {
            tracing::warn!(error = %e, db = %frames_db_path.display(), "failed to open frames database");
        }
    }

    let ems_handle = match EmsService::open(&ems_db_path).await {
        Ok(svc) => {
            tracing::debug!(db = %ems_db_path.display(), "EMS database opened");
            Some(svc.handle())
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to open EMS database");
            None
        }
    };

    // Register EMS with kernel singleton for syscall access
    if let Some(ref ems) = ems_handle
        && let Some(k) = Kernel::get()
    {
        k.set_ems(ems.clone());
    }

    let snapshot =
        abbot::runtime::SnapshotManager::new(paths.home.clone(), Some(store.clone())).await;

    let mut hand = HandService::new(store.clone(), paths.home.clone(), snapshot.clone());
    if let Some(ref ems) = ems_handle {
        hand = hand.with_ems(ems.clone());
    }
    Arc::new(hand).start();

    // Start head pool (kernel need queue dispatches needs to these)
    let head_cfg = HeadConfig::from_config();
    let session_locks = SessionWriteLocks::new();
    tracing::info!(pool_size = head_cfg.pool_size, "starting head pool");
    for i in 0..head_cfg.pool_size {
        let head_id = format!("head-{}", i);
        let mut head = HeadService::new(
            store.clone(),
            paths.home.clone(),
            &head_id,
            vec![Scope::main()],
            snapshot.clone(),
            session_locks.clone(),
        );
        if let Some(ref ems) = ems_handle {
            head = head.with_ems(ems.clone());
        }
        Arc::new(head).start();
    }

    let coordinator = RoomCoordinator::new(
        store.clone(),
        DEFAULT_HEAD_ID,
        vec![Scope::main()],
        paths.home.clone(),
    )
    .with_conclave_on_boot(cli.conclave);

    Arc::new(coordinator).start();

    let mind_loop = MindLoop::new(store.clone());
    Arc::new(mind_loop).start();

    // Determine web dist path (config override, otherwise relative to manifest/exe)
    let web_dist = AppConfig::global()
        .server
        .web_dist
        .as_deref()
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                        .unwrap_or_else(|| PathBuf::from("."))
                });
            manifest_dir.join("..").join("web").join("dist")
        });

    Server::new(store.clone(), DEFAULT_HEAD_ID)
        .with_addr(&bind_addr)
        .with_web_dist(web_dist)
        .spawn();

    // Spawn frontend if requested
    let mut frontend_child: Option<tokio::process::Child> = None;
    if let Some(ref fe) = frontend {
        // Wait for server to be ready
        let health_url = format!("http://{}/health", bind_addr);
        for _ in 0..50 {
            if reqwest::get(&health_url).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        match fe {
            RunFrontend::Opencode { args } => {
                // Update opencode config with current address
                if let Err(e) = update_opencode_config(&bind_addr) {
                    tracing::warn!(error = %e, "failed to update opencode config");
                }

                let model_arg = "abbot/abbot/default";
                tracing::info!(model = model_arg, "launching opencode");

                match tokio::process::Command::new("opencode")
                    .arg("-m")
                    .arg(model_arg)
                    .args(args)
                    .spawn()
                {
                    Ok(c) => frontend_child = Some(c),
                    Err(e) => {
                        tracing::error!(error = %e, "failed to spawn opencode");
                        return Err(e.into());
                    }
                }
            }
            RunFrontend::Claude { args } => {
                let base_url = format!("http://{}", bind_addr);
                tracing::info!(base_url = %base_url, "launching claude");

                match tokio::process::Command::new("claude")
                    .env("ANTHROPIC_BASE_URL", &base_url)
                    .env("ANTHROPIC_API_KEY", "abbot")
                    .args(args)
                    .spawn()
                {
                    Ok(c) => frontend_child = Some(c),
                    Err(e) => {
                        tracing::error!(error = %e, "failed to spawn claude");
                        return Err(e.into());
                    }
                }
            }
            RunFrontend::Web => {
                let url = format!("http://{}", bind_addr);
                tracing::info!(url = %url, "opening browser");

                #[cfg(target_os = "macos")]
                let result = std::process::Command::new("open").arg(&url).spawn();
                #[cfg(target_os = "linux")]
                let result = std::process::Command::new("xdg-open").arg(&url).spawn();
                #[cfg(target_os = "windows")]
                let result = std::process::Command::new("cmd")
                    .args(["/C", "start", &url])
                    .spawn();

                if let Err(e) = result {
                    tracing::warn!(error = %e, "failed to open browser");
                }
            }
            RunFrontend::Prompt { .. } => {}
        }
    }

    let exit = cli.exit;

    if let Some(ref prompt) = initial_prompt {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".into());
        };

        let thread_id = uuid::Uuid::new_v4();
        let scope = Scope::main();

        let _ = k.sigcalls().open(scope.as_str(), thread_id).await;

        // Best-effort chat ingress (logs + enqueues need internally).
        let dispatcher = k.dispatcher().await;
        let req = abbot::kernel::Frame::req(
            "chat:message",
            serde_json::json!({
                "scope": scope.as_str(),
                "reply_to": thread_id.to_string(),
                "content": prompt,
            }),
        )
        .with_actor("user");
        let mut rx = dispatcher.dispatch(
            req,
            k.workspace().to_path_buf(),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx.recv().await;

        if exit {
            // Wait for the reply stream to terminate.
            let mut reply_rx = k.sigcalls().open(scope.as_str(), thread_id).await;
            while let Some(frame) = reply_rx.recv().await {
                if matches!(
                    frame.op,
                    abbot::kernel::FrameOp::Done | abbot::kernel::FrameOp::Error
                ) {
                    break;
                }
            }
            tracing::info!("exiting (--exit mode)");
            return Ok(());
        }
    }

    // Keep the daemon alive.
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutdown requested");
        }
        status = async {
            match frontend_child.as_mut() {
                Some(child) => child.wait().await,
                None => std::future::pending().await,
            }
        } => {
            match status {
                Ok(s) if s.success() => tracing::info!("frontend exited"),
                Ok(s) => tracing::info!(code = ?s.code(), "frontend exited"),
                Err(e) => tracing::warn!(error = %e, "frontend wait failed"),
            }
        }
    }

    Ok(())
}

// =============================================================================
// LOGGING
// =============================================================================

fn init_logging(log_format: &str, file: Option<std::fs::File>, ansi: bool) {
    use tracing_subscriber::fmt::format::FmtSpan;

    let format = log_format.trim().to_ascii_lowercase();

    match (file, format.as_str()) {
        (Some(f), "compact") => tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(f))
            .compact()
            .with_ansi(ansi)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .init(),
        (None, "compact") => tracing_subscriber::fmt()
            .compact()
            .with_ansi(ansi)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .init(),

        (Some(f), "pretty") => tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(f))
            .pretty()
            .with_ansi(ansi)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .init(),
        (None, "pretty") => tracing_subscriber::fmt()
            .pretty()
            .with_ansi(ansi)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .init(),

        (Some(f), _) => tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(f))
            .with_ansi(ansi)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .init(),
        (None, _) => tracing_subscriber::fmt()
            .with_ansi(ansi)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .init(),
    };
}
