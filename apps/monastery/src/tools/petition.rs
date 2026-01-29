use async_trait::async_trait;
use super::{Tool, ExecutionContext};

pub struct PetitionTool;

impl PetitionTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for PetitionTool {
    fn name(&self) -> &str {
        "petition"
    }

    fn description(&self) -> &str {
        "Send a petition to the human via Reminders with a summary of open issues"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let message = args.trim();
        if message.is_empty() {
            return "error: petition requires a message".to_string();
        }

        let cwd = ctx.cwd.lock().unwrap().clone();

        // Get open issues from GitHub
        let issues = match get_open_issues(&cwd).await {
            Ok(issues) => issues,
            Err(e) => return format!("error: failed to get issues: {}", e),
        };

        // Build the reminder body
        let issue_summary = if issues.is_empty() {
            "No open issues.".to_string()
        } else {
            format!("{} open issue(s)", issues.len())
        };

        let repo_url = get_repo_url(&cwd).await.unwrap_or_default();
        let issues_url = if repo_url.is_empty() {
            String::new()
        } else {
            format!("{}/issues", repo_url)
        };

        let reminder_title = format!("Monastery petition: {}", truncate(message, 50));
        let reminder_body = format!(
            "{}\n\n{}\n\n{}",
            message,
            issue_summary,
            issues_url
        );

        // Create the reminder
        match create_reminder(&reminder_title, &reminder_body).await {
            Ok(_) => format!(
                "Petition sent.\n{}\nReminder created with link to issues.",
                issue_summary
            ),
            Err(e) => format!("error: failed to create reminder: {}", e),
        }
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

async fn get_open_issues(cwd: &std::path::Path) -> Result<Vec<String>, String> {
    let output = tokio::process::Command::new("gh")
        .args(["issue", "list", "--state", "open", "--limit", "20", "--json", "number,title"])
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON to get issue titles
    let issues: Vec<String> = serde_json::from_str::<Vec<serde_json::Value>>(&stdout)
        .unwrap_or_default()
        .iter()
        .filter_map(|v| {
            let num = v.get("number")?.as_i64()?;
            let title = v.get("title")?.as_str()?;
            Some(format!("#{}: {}", num, title))
        })
        .collect();

    Ok(issues)
}

async fn get_repo_url(cwd: &std::path::Path) -> Result<String, String> {
    let output = tokio::process::Command::new("gh")
        .args(["repo", "view", "--json", "url", "-q", ".url"])
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err("failed to get repo url".to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn create_reminder(title: &str, body: &str) -> Result<(), String> {
    // Escape quotes for AppleScript
    let title_escaped = title.replace('\\', "\\\\").replace('"', "\\\"");
    let body_escaped = body.replace('\\', "\\\\").replace('"', "\\\"");

    let script = format!(
        r#"tell application "Reminders"
            set targetList to list "Reminders"
            tell targetList
                make new reminder with properties {{name:"{}", body:"{}"}}
            end tell
        end tell"#,
        title_escaped,
        body_escaped
    );

    let output = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello");
    }
}
