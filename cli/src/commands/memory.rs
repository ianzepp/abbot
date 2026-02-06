//! Memory command - Index/stats/search/wipe memory embeddings

use std::path::PathBuf;

use clap::Subcommand;

use crate::config;
use crate::error::CliError;

#[derive(Debug, Subcommand, Clone)]
pub enum MemoryAction {
    /// Index transcript files from a directory
    Index {
        /// Directory containing transcript files
        path: PathBuf,
    },
    /// Show memory index statistics
    Stats,
    /// Search memory for a query
    Search {
        /// Search query
        query: Vec<String>,
    },
    /// Wipe all memory data
    Wipe,
}

pub async fn run(cli_config: Option<PathBuf>, action: MemoryAction) -> Result<(), CliError> {
    use abbot::recall::{Indexer, Ollama, Search, ensure_schema as ensure_recall_schema};
    use abbot::runtime::AppConfig;
    use abbot::runtime::app_config::WorkspacePaths;

    config::init_app_config(cli_config.as_deref());

    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }

    let workspace = AppConfig::global()
        .workspace_path()
        .map_err(|e| CliError::General(format!("workspace configuration error: {}", e)))?;
    let paths = WorkspacePaths::new(workspace);
    let recall_db_path = paths.recall_db.clone();
    let conn =
        rusqlite::Connection::open(&recall_db_path).map_err(|e| CliError::General(e.to_string()))?;
    ensure_recall_schema(&conn).map_err(|e| CliError::General(e.to_string()))?;

    match action {
        MemoryAction::Index { path } => {
            let ollama = Ollama::local();
            let indexer = Indexer::new(conn, ollama);

            println!("Indexing {}...", path.display());
            let start = std::time::Instant::now();

            let result = indexer
                .index_directory(&path)
                .await
                .map_err(|e| CliError::General(e.to_string()))?;

            println!(
                "Done in {:.1}s: {} indexed, {} skipped, {} errors",
                start.elapsed().as_secs_f32(),
                result.indexed,
                result.skipped,
                result.errors
            );
        }

        MemoryAction::Stats => {
            let ollama = Ollama::local();
            let search = Search::new(conn, ollama);
            let stats = search
                .stats()
                .map_err(|e| CliError::General(e.to_string()))?;

            println!("Recall index stats:");
            println!("  Transcripts: {}", stats.transcripts);
            println!("  Chunks:      {}", stats.chunks);
            println!("  Vectors:     {}", stats.vectors);
        }

        MemoryAction::Search { query } => {
            let query_str = query.join(" ");
            if query_str.is_empty() {
                eprintln!("usage: abbot memory search <query>");
                std::process::exit(1);
            }

            let ollama = Ollama::local();
            let search = Search::new(conn, ollama);

            println!("Searching for: {}\n", query_str);
            let results = search
                .query(&query_str, 5)
                .await
                .map_err(|e| CliError::General(e.to_string()))?;

            for (i, r) in results.iter().enumerate() {
                println!(
                    "{}. [dist={:.3}] {} ({})",
                    i + 1,
                    r.distance,
                    r.source,
                    r.file_path
                );
                println!("   project: {:?}", r.project_path);
                println!("   ---");
                let preview: String = r.content.chars().take(200).collect();
                println!("   {}", preview.replace('\n', "\n   "));
                println!();
            }
        }

        MemoryAction::Wipe => {
            conn.execute("DELETE FROM chunk_vectors", [])
                .map_err(|e| CliError::General(e.to_string()))?;
            conn.execute("DELETE FROM chunks", [])
                .map_err(|e| CliError::General(e.to_string()))?;
            conn.execute("DELETE FROM transcripts", [])
                .map_err(|e| CliError::General(e.to_string()))?;
            println!("Memory wiped.");
        }
    }

    Ok(())
}
