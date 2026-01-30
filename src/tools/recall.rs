// RecallTool searches semantic memory for relevant past conversations.
//
// Uses vector similarity to find chunks from indexed transcripts that
// match the query. Returns formatted excerpts with source attribution.

use super::{ExecutionContext, Tool};
use crate::memory::Search;

pub struct RecallTool {
    search: Search,
}

impl RecallTool {
    pub fn new(search: Search) -> Self {
        Self { search }
    }
}

#[async_trait::async_trait]
impl Tool for RecallTool {
    fn name(&self) -> &str {
        "recall"
    }

    fn description(&self) -> &str {
        "Search memory for past conversations (e.g. !recall why did we choose JWT)"
    }

    async fn execute(&self, args: &str, _ctx: &ExecutionContext) -> String {
        let query = args.trim();
        if query.is_empty() {
            return "usage: !recall <query>".to_string();
        }

        let limit = 5;

        match self.search.query(query, limit).await {
            Ok(results) if results.is_empty() => "no relevant memories found".to_string(),
            Ok(results) => {
                let mut output = Vec::new();
                output.push(format!("Found {} relevant memories:\n", results.len()));

                for (i, r) in results.iter().enumerate() {
                    let project = r
                        .project_path
                        .as_ref()
                        .map(|p| {
                            p.split('/')
                                .last()
                                .unwrap_or(p)
                        })
                        .unwrap_or("unknown");

                    let date = chrono::DateTime::from_timestamp(r.started_at, 0)
                        .map(|dt| dt.format("%Y-%m-%d").to_string())
                        .unwrap_or_else(|| "unknown".to_string());

                    output.push(format!(
                        "--- [{}/{}] {} ({}) ---",
                        i + 1,
                        results.len(),
                        project,
                        date
                    ));

                    let preview: String = r.content.chars().take(800).collect();
                    output.push(preview);
                    output.push(String::new());
                }

                output.join("\n")
            }
            Err(e) => format!("recall error: {}", e),
        }
    }
}
