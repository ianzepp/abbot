use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use crate::bus::{Hub, respond};
use crate::history::Store;

const POLL_INTERVAL: Duration = Duration::from_secs(120);

pub struct Poller {
    hub: Arc<RwLock<Hub>>,
    store: Arc<Store>,
    repo: String,
}

impl Poller {
    pub fn new(hub: Arc<RwLock<Hub>>, store: Arc<Store>, repo: String) -> Self {
        Self { hub, store, repo }
    }

    pub async fn run(&self) {
        let mut interval = tokio::time::interval(POLL_INTERVAL);

        loop {
            interval.tick().await;

            if let Err(e) = self.poll_comments().await {
                tracing::warn!(error = %e, "failed to poll GitHub comments");
            }
        }
    }

    async fn poll_comments(&self) -> Result<(), String> {
        // Get the last seen comment ID from the database
        let last_seen = self.store.get_meta("github_last_comment_id")
            .unwrap_or_default()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        // Fetch recent issue comments
        let comments = self.fetch_comments().await?;

        let mut max_id = last_seen;
        let mut new_comments = Vec::new();

        for comment in comments {
            if comment.id > last_seen {
                new_comments.push(comment.clone());
                if comment.id > max_id {
                    max_id = comment.id;
                }
            }
        }

        // Sort by ID ascending so we publish in order
        new_comments.sort_by_key(|c| c.id);

        // Publish new comments to channels
        for comment in new_comments {
            let channel = format!("#{}#{}", self.repo, comment.issue_number);

            // Create channel if needed
            self.hub.write().await.create_channel(&channel);

            // Publish the comment
            let msg = respond::chat(&comment.author, &channel, &comment.body);
            self.hub.read().await.publish(&channel, msg);

            tracing::info!(
                channel = %channel,
                author = %comment.author,
                "published GitHub comment"
            );
        }

        // Update last seen ID
        if max_id > last_seen {
            let _ = self.store.set_meta("github_last_comment_id", &max_id.to_string());
        }

        Ok(())
    }

    async fn fetch_comments(&self) -> Result<Vec<Comment>, String> {
        // Fetch recent issue comments via gh api (last 30 comments, sorted by updated)
        let output = tokio::process::Command::new("gh")
            .args([
                "api",
                &format!("repos/{}/issues/comments?sort=updated&direction=desc&per_page=30", self.repo),
            ])
            .output()
            .await
            .map_err(|e| e.to_string())?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(stderr.to_string());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: Vec<serde_json::Value> = serde_json::from_str(&stdout)
            .map_err(|e| e.to_string())?;

        let mut comments = Vec::new();

        for item in json {
            let id = item.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let issue_url = item.get("issue_url").and_then(|v| v.as_str()).unwrap_or("");
            let author = item.get("user")
                .and_then(|u| u.get("login"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let body = item.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();

            // Extract issue number from URL like "https://api.github.com/repos/owner/repo/issues/123"
            let issue_number = issue_url
                .rsplit('/')
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);

            if id > 0 && issue_number > 0 {
                comments.push(Comment {
                    id,
                    issue_number,
                    author,
                    body,
                });
            }
        }

        Ok(comments)
    }
}

#[derive(Clone)]
struct Comment {
    id: u64,
    issue_number: u64,
    author: String,
    body: String,
}
