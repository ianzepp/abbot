use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use chrono::{Local, TimeZone};

// Import the history store
use abbot::history::Store;

#[derive(Parser)]
#[command(name = "abbot")]
#[command(about = "CLI for the AI monastery - introspect monk activity and messages")]
#[command(version)]
struct Cli {
    /// Path to the monastery directory (contains monks/, hermitage/, database)
    #[arg(short, long, env = "ABBOT_MONASTERY", default_value = ".", global = true)]
    monastery: PathBuf,

    /// Path to the abbot database (relative to monastery, or absolute)
    #[arg(short, long, env = "ABBOT_DB", default_value = "abbot.db", global = true)]
    db: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

impl Cli {
    /// Get the database path (absolute or relative to monastery)
    fn db_path(&self) -> PathBuf {
        if self.db.is_absolute() {
            self.db.clone()
        } else {
            self.monastery.join(&self.db)
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Show monastery overview - monks, channels, recent activity
    Status,

    /// Stream recent messages from a channel (default: #general)
    Tail {
        /// Channel name (default: #general)
        #[arg(default_value = "#general")]
        channel: String,

        /// Number of messages to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Query message history
    History {
        /// Channel name (default: #general)
        #[arg(default_value = "#general")]
        channel: String,

        /// Filter by monk/sender
        #[arg(short, long)]
        monk: Option<String>,

        /// Show messages since (e.g., 1h, 30m, 1d)
        #[arg(short, long)]
        since: Option<String>,

        /// Number of messages to show
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },

    /// List all monks and their status
    Monks,

    /// Deep dive into a specific monk
    Monk {
        /// Monk ID/name
        name: String,
    },

    /// Search messages
    Search {
        /// Search query
        query: String,

        /// Channel to search (default: all channels)
        #[arg(short, long)]
        channel: Option<String>,

        /// Number of results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Show recent tool calls
    Tools {
        /// Filter by monk
        #[arg(short, long)]
        monk: Option<String>,

        /// Number of tool calls to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// List all channels
    Channels,
}

fn main() {
    let cli = Cli::parse();

    // Determine database path
    let db_path = cli.db_path();

    // Open database
    let store = match Store::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error opening database at {}: {}", db_path.display(), e);
            std::process::exit(1);
        }
    };

    // Print monastery header
    print_monastery_header(&db_path);

    // Execute command
    match cli.command {
        Commands::Status => cmd_status(&store),
        Commands::Tail { channel, limit } => cmd_tail(&store, &channel, limit),
        Commands::History { channel, monk, since, limit } => {
            cmd_history(&store, &channel, monk, since, limit)
        }
        Commands::Monks => cmd_monks(&store),
        Commands::Monk { name } => cmd_monk(&store, &name),
        Commands::Search { query, channel, limit } => {
            cmd_search(&store, &query, channel, limit)
        }
        Commands::Tools { monk, limit } => cmd_tools(&store, monk, limit),
        Commands::Channels => cmd_channels(&store),
    }
}

fn print_monastery_header(db_path: &PathBuf) {
    let now = Local::now();
    println!("📋 Monastery: {}", std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."))
        .display());
    println!("📋 Database: {}", db_path.display());
    println!("📋 Time: {}", now.to_rfc3339());
    println!();
}

fn cmd_status(store: &Store) {
    // Get all monks
    let monks = match store.list_monks() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error listing monks: {}", e);
            return;
        }
    };

    println!("🔔 Monastery Status");
    println!("   {} monks registered", monks.len());

    // Get recent activity from #general
    let recent = match store.recent("#general", 10) {
        Ok(m) => m,
        Err(_) => vec![],
    };

    if !recent.is_empty() {
        println!();
        println!("📋 Recent activity in #general:");
        for msg in recent.iter().rev().take(5) {
            let time = format_timestamp(&msg.timestamp);
            let preview = match msg.text() {
                Some(t) if t.len() > 60 => format!("{}...", &t[..60]),
                Some(t) => t.to_string(),
                None => format!("[{:?}]", msg.op),
            };
            println!("   [{}] {}: {}", time, msg.sender, preview);
        }
    }

    // Show each monk's status
    println!();
    for (monk_id, model) in &monks {
        let self_content = store.get_monk_self(monk_id).unwrap_or_default();
        let channels = store.list_monk_channels(monk_id).unwrap_or_default();

        println!("🧘 {} (model: {})", monk_id, model);

        // Extract "Next Actions" or "Observations" from self if present
        if !self_content.is_empty() {
            let preview = extract_section_preview(&self_content, "Next Actions");
            if !preview.is_empty() {
                println!("   📋 Next: {}", preview);
            }
        }

        // Show active channels
        if !channels.is_empty() {
            println!("   🔔 Channels: {}", channels.join(", "));
        }

        // Recent tool calls count
        let one_hour_ago = SystemTime::now() - Duration::from_secs(3600);
        let since_ms = one_hour_ago
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let tool_stats = store.tool_call_stats(monk_id, since_ms).unwrap_or_default();
        let total_calls: i64 = tool_stats.iter().map(|(_, _, count)| count).sum();
        if total_calls > 0 {
            println!("   🛠️  {} tool calls in last hour", total_calls);
        }
    }
}

fn cmd_tail(store: &Store, channel: &str, limit: usize) {
    let messages = match store.recent(channel, limit) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error reading channel {}: {}", channel, e);
            return;
        }
    };

    println!("🔔 Channel: {} (showing last {} messages)", channel, limit);
    println!();

    for msg in messages {
        print_message(&msg);
    }
}

fn cmd_history(store: &Store, channel: &str, monk: Option<String>, since: Option<String>, limit: usize) {
    let since_ms = since.as_ref().and_then(|s| parse_duration(s));

    let messages = if let Some(ref sender) = monk {
        match store.recent_from(channel, &sender, limit) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Error querying history: {}", e);
                return;
            }
        }
    } else if let Some(since_ms) = since_ms {
        // Query with time filter - we'll filter in code for now
        match store.recent(channel, limit * 2) {
            Ok(m) => m.into_iter()
                .filter(|msg| {
                    let msg_ms = msg.timestamp
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as i64;
                    msg_ms >= since_ms
                })
                .take(limit)
                .collect(),
            Err(e) => {
                eprintln!("Error querying history: {}", e);
                return;
            }
        }
    } else {
        match store.recent(channel, limit) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Error querying history: {}", e);
                return;
            }
        }
    };

    println!("📋 History: {} ({} messages)", channel, messages.len());
    if let Some(s) = since {
        println!("   Filter: since {}", s);
    }
    if let Some(m) = monk {
        println!("   Filter: from {}", m);
    }
    println!();

    for msg in messages {
        print_message(&msg);
    }
}

fn cmd_monks(store: &Store) {
    let monks = match store.list_monks() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error listing monks: {}", e);
            return;
        }
    };

    println!("🧘 Monks ({})\n", monks.len());

    for (monk_id, model) in monks {
        println!("────────────────────────────────────────");
        println!("🧘 {} (model: {})", monk_id, model);

        // Self layer
        let self_content = store.get_monk_self(&monk_id).unwrap_or_default();
        if !self_content.is_empty() {
            println!();
            println!("💭 Self (Layer 2):");
            for line in self_content.lines().take(10) {
                println!("   {}", line);
            }
            if self_content.lines().count() > 10 {
                println!("   ... ({} more lines)", self_content.lines().count() - 10);
            }
        }

        // Workspaces
        let channels = store.list_monk_channels(&monk_id).unwrap_or_default();
        if !channels.is_empty() {
            println!();
            for channel in channels {
                let ws = store.get_workspace(&monk_id, &channel).unwrap_or_default();
                if !ws.is_empty() {
                    println!("📋 Workspace for {}:", channel);
                    for line in ws.lines().take(5) {
                        println!("   {}", line);
                    }
                    if ws.lines().count() > 5 {
                        println!("   ... ({} more lines)", ws.lines().count() - 5);
                    }
                    println!();
                }
            }
        }

        // Garden
        let garden = store.garden_list(&monk_id).unwrap_or_default();
        if !garden.is_empty() {
            println!("🌱 Garden ({} items):", garden.len());
            for (id, content, age_days) in garden.iter().take(5) {
                let preview = if content.len() > 50 {
                    format!("{}...", &content[..50])
                } else {
                    content.clone()
                };
                println!("   [{}] {} ({}d)", id, preview, age_days);
            }
            if garden.len() > 5 {
                println!("   ... ({} more items)", garden.len() - 5);
            }
        }

        println!();
    }
}

fn cmd_monk(store: &Store, name: &str) {
    // Check if monk exists
    let exists = match store.monk_exists(name) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error checking monk: {}", e);
            return;
        }
    };

    if !exists {
        eprintln!("🚫 Monk '{}' not found", name);
        return;
    }

    let monks = match store.list_monks() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error listing monks: {}", e);
            return;
        }
    };

    let model = monks.iter()
        .find(|(id, _)| id == name)
        .map(|(_, m)| m.clone())
        .unwrap_or_default();

    println!("🧘 Monk: {} (model: {})", name, model);
    println!("══════════════════════════════════════════");

    // Full self layer
    let self_content = store.get_monk_self(name).unwrap_or_default();
    println!();
    println!("💭 Self (Layer 2 - Identity & Mission):");
    println!("─────────────────────────────────────────");
    if self_content.is_empty() {
        println!("   (empty)");
    } else {
        for line in self_content.lines() {
            println!("   {}", line);
        }
    }

    // All workspaces
    let channels = store.list_monk_channels(name).unwrap_or_default();
    println!();
    println!("📋 Workspaces (Layer 3 - Channel Context):");
    println!("─────────────────────────────────────────");
    for channel in &channels {
        let ws = store.get_workspace(name, channel).unwrap_or_default();
        println!();
        println!("   Channel: {}", channel);
        if ws.is_empty() {
            println!("   (empty)");
        } else {
            for line in ws.lines() {
                println!("   {}", line);
            }
        }
    }

    // Full garden
    let garden = store.garden_list(name).unwrap_or_default();
    println!();
    println!("🌱 Garden (Knowledge Collection):");
    println!("─────────────────────────────────────────");
    if garden.is_empty() {
        println!("   (empty)");
    } else {
        for (id, content, age_days) in garden {
            println!();
            println!("   [{}] ({} days old)", id, age_days);
            for line in content.lines() {
                println!("   {}", line);
            }
        }
    }

    // Recent tool calls
    let tool_calls = store.recent_tool_calls(name, 20).unwrap_or_default();
    println!();
    println!("🛠️ Recent Tool Calls:");
    println!("─────────────────────────────────────────");
    if tool_calls.is_empty() {
        println!("   (none)");
    } else {
        for call in tool_calls {
            let time = format_timestamp_millis(call.timestamp);
            let status = if call.success { "✓" } else { "✗" };
            println!();
            println!("   [{}] {} {} ({}ms)", time, status, call.tool, call.duration_ms);
            if let Some(reason) = call.reason {
                println!("   Reason: {}", reason);
            }
            let content_preview = if call.content.len() > 60 {
                format!("{}...", &call.content[..60])
            } else {
                call.content
            };
            println!("   Input: {}", content_preview);
        }
    }
}

fn cmd_search(store: &Store, query: &str, channel: Option<String>, limit: usize) {
    let channels_to_search: Vec<String> = if let Some(ch) = channel {
        vec![ch]
    } else {
        // Get all channels by querying distinct channel names
        // For now, we'll search #general as default
        vec!["#general".to_string()]
    };

    println!("🔍 Search: \"{}\"\n", query);

    let mut total_found = 0;
    for ch in channels_to_search {
        let results = match store.search(&ch, query, limit) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error searching {}: {}", ch, e);
                continue;
            }
        };

        let count = results.len();
        if count > 0 {
            println!("📋 Channel: {} ({} results)", ch, count);
            for msg in &results {
                print_message(msg);
            }
            total_found += count;
            println!();
        }
    }

    println!("─────────────────────────────────────────");
    println!("Total results: {}", total_found);
}

fn cmd_tools(store: &Store, monk: Option<String>, limit: usize) {
    if let Some(monk_id) = monk {
        // Show tools for specific monk
        let calls = match store.recent_tool_calls(&monk_id, limit) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error fetching tool calls: {}", e);
                return;
            }
        };

        println!("🛠️ Tool calls for {} (showing {})", monk_id, calls.len());
        println!();

        for call in calls {
            print_tool_call(&call);
        }
    } else {
        // Show summary across all monks
        let monks = match store.list_monks() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Error listing monks: {}", e);
                return;
            }
        };

        println!("🛠️ Recent Tool Activity (last hour)\n");

        let one_hour_ago = SystemTime::now() - Duration::from_secs(3600);
        let since_ms = one_hour_ago
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        for (monk_id, _) in monks {
            let stats = match store.tool_call_stats(&monk_id, since_ms) {
                Ok(s) => s,
                Err(_) => continue,
            };

            if !stats.is_empty() {
                println!("🧘 {}:", monk_id);
                let total: i64 = stats.iter().map(|(_, _, count)| count).sum();
                println!("   Total: {} calls", total);

                // Show top tools
                for (tool, reason, count) in stats.iter().take(5) {
                    let reason_str = if reason.is_empty() {
                        "".to_string()
                    } else {
                        format!(" - {}", reason)
                    };
                    println!("   {}: {}{}", tool, count, reason_str);
                }
                println!();
            }
        }
    }
}

fn cmd_channels(store: &Store) {
    let channels = match store.list_channels() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error listing channels: {}", e);
            return;
        }
    };

    println!("📋 Channels ({} with messages)\n", channels.len());

    for (channel, count) in &channels {
        println!("🔔 {} ({} messages)", channel, count);
    }

    // Also show empty standard channels if not in list
    let known = ["#general", "#ping"];
    let empty: Vec<_> = known.iter()
        .filter(|k| !channels.iter().any(|(c, _)| c == *k))
        .collect();

    if !empty.is_empty() {
        for ch in empty {
            println!("🔔 {} (0 messages)", ch);
        }
    }
}

fn print_message(msg: &abbot::bus::Message) {
    let time = format_timestamp(&msg.timestamp);

    match msg.op {
        abbot::bus::MessageOp::Chat => {
            let text = msg.text().unwrap_or("(no text)");
            if msg.sender == "_user" || msg.sender == "ianzepp" {
                println!("👤 [{}] {}: {}", time, msg.sender, text);
            } else {
                println!("🤖 [{}] {}: {}", time, msg.sender, text);
            }
        }
        abbot::bus::MessageOp::Exec => {
            if let abbot::bus::MessageData::Exec { tool, args } = &msg.data {
                println!("🛠️  [{}] {}: {} {}", time, msg.sender, tool, args);
            }
        }
        abbot::bus::MessageOp::Ok => {
            if let Some(text) = msg.text() {
                let preview = if text.len() > 80 {
                    format!("{}...", &text[..80])
                } else {
                    text.to_string()
                };
                println!("✓ [{}] {}: {}", time, msg.sender, preview);
            }
        }
        abbot::bus::MessageOp::Error => {
            if let Some(text) = msg.text() {
                println!("✗ [{}] {}: {}", time, msg.sender, text);
            }
        }
        abbot::bus::MessageOp::Item => {
            if let Some(text) = msg.text() {
                let preview = if text.len() > 80 {
                    format!("{}...", &text[..80])
                } else {
                    text.to_string()
                };
                println!("📦 [{}] {}: {}", time, msg.sender, preview);
            }
        }
        abbot::bus::MessageOp::Ping => {
            println!("🔔 [{}] {}: ping", time, msg.sender);
        }
        _ => {
            println!("📋 [{}] {}: {:?}", time, msg.sender, msg.op);
        }
    }
}

fn print_tool_call(call: &abbot::history::ToolCallRecord) {
    let time = format_timestamp_millis(call.timestamp);
    let status = if call.success { "✓" } else { "✗" };

    println!("─────────────────────────────────────────");
    println!("🛠️  [{}] {} {} (batch: {})", time, status, call.tool, call.batch_id);
    println!("   Duration: {}ms", call.duration_ms);

    if let Some(reason) = &call.reason {
        println!("   Reason: {}", reason);
    }

    let content = if call.content.len() > 100 {
        format!("{}...", &call.content[..100])
    } else {
        call.content.clone()
    };
    println!("   Input: {}", content);

    let output = if call.output.len() > 100 {
        format!("{}...", &call.output[..100])
    } else {
        call.output.clone()
    };
    println!("   Output: {}", output);
    println!();
}

fn format_timestamp(ts: &SystemTime) -> String {
    let ms = ts
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    format_timestamp_millis(ms)
}

fn format_timestamp_millis(ms: i64) -> String {
    let secs = ms / 1000;
    let datetime = Local.timestamp_opt(secs, 0).single();
    match datetime {
        Some(dt) => dt.format("%H:%M:%S").to_string(),
        None => format!("{}ms", ms),
    }
}

fn parse_duration(s: &str) -> Option<i64> {
    // Parse things like "1h", "30m", "1d"
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: u64 = num_str.parse().ok()?;

    let duration_secs = match unit {
        "s" => num,
        "m" => num * 60,
        "h" => num * 3600,
        "d" => num * 86400,
        _ => return None,
    };

    let since = SystemTime::now() - Duration::from_secs(duration_secs);
    Some(since.duration_since(UNIX_EPOCH).unwrap().as_millis() as i64)
}

fn extract_section_preview(content: &str, section: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.to_lowercase().contains(&section.to_lowercase()) {
            // Get next non-empty line
            for j in (i + 1)..lines.len() {
                let next_line = lines[j].trim();
                if !next_line.is_empty() && !next_line.starts_with("##") {
                    let clean = next_line.trim_start_matches("- ").trim_start_matches("1. ");
                    if clean.len() > 60 {
                        return format!("{}...", &clean[..60]);
                    } else {
                        return clean.to_string();
                    }
                }
            }
        }
    }
    String::new()
}
