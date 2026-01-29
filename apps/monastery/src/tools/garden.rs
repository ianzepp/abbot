use std::path::PathBuf;
use tokio::fs;
use chrono::{DateTime, Utc};
use super::{Tool, ExecutionContext};

pub struct GardenTool;

impl GardenTool {
    pub fn new() -> Self {
        Self
    }

    fn garden_path(ctx: &ExecutionContext) -> PathBuf {
        let cwd = ctx.cwd.lock().unwrap();
        cwd.join("garden")
    }

    fn plant_path(ctx: &ExecutionContext, name: &str) -> PathBuf {
        let sanitized = sanitize_name(name);
        Self::garden_path(ctx).join(format!("{}.md", sanitized))
    }
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect::<String>()
        .to_lowercase()
}

#[async_trait::async_trait]
impl Tool for GardenTool {
    fn name(&self) -> &str {
        "garden"
    }

    fn description(&self) -> &str {
        "Tend your garden of memories"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let args = args.trim();

        if args == "list" || args.is_empty() {
            return self.list(ctx).await;
        }

        if let Some(name) = args.strip_prefix("view ") {
            return self.view(name.trim(), ctx).await;
        }

        if let Some(rest) = args.strip_prefix("seed ") {
            let parts: Vec<&str> = rest.splitn(2, ' ').collect();
            if parts.len() == 2 {
                return self.seed(parts[0].trim(), parts[1].trim(), ctx).await;
            }
            return "usage: seed <name> <initial thought>".to_string();
        }

        if let Some(rest) = args.strip_prefix("grow ") {
            let parts: Vec<&str> = rest.splitn(2, ' ').collect();
            if parts.len() == 2 {
                return self.grow(parts[0].trim(), parts[1].trim(), ctx).await;
            }
            return "usage: grow <name> <observation>".to_string();
        }

        if let Some(rest) = args.strip_prefix("prune ") {
            let parts: Vec<&str> = rest.splitn(2, ' ').collect();
            if parts.len() == 2 {
                return self.prune(parts[0].trim(), parts[1].trim(), ctx).await;
            }
            return "usage: prune <name> <compacted content>".to_string();
        }

        if let Some(name) = args.strip_prefix("uproot ") {
            return self.uproot(name.trim(), ctx).await;
        }

        "usage: garden [list|view <name>|seed <name> <thought>|grow <name> <thought>|prune <name> <new content>|uproot <name>]".to_string()
    }
}

impl GardenTool {
    async fn list(&self, ctx: &ExecutionContext) -> String {
        let garden = Self::garden_path(ctx);

        if !garden.exists() {
            return "your garden is empty. plant a seed with: garden seed <name> <thought>".to_string();
        }

        let mut entries = match fs::read_dir(&garden).await {
            Ok(e) => e,
            Err(e) => return format!("error reading garden: {}", e),
        };

        let mut plants = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                let name = path.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();

                let metadata = fs::metadata(&path).await.ok();
                let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
                let modified = metadata.and_then(|m| m.modified().ok());

                plants.push((name, size, modified));
            }
        }

        if plants.is_empty() {
            return "your garden is empty. plant a seed with: garden seed <name> <thought>".to_string();
        }

        // Sort by name
        plants.sort_by(|a, b| a.0.cmp(&b.0));

        let mut lines = vec![format!("your garden ({} plants):", plants.len())];
        for (name, size, modified) in plants {
            let size_str = format_size(size);
            let age_str = modified
                .map(|m| format_age(m))
                .unwrap_or_else(|| "unknown".to_string());
            lines.push(format!("  {} ({}, {})", name, size_str, age_str));
        }

        lines.join("\n")
    }

    async fn view(&self, name: &str, ctx: &ExecutionContext) -> String {
        let path = Self::plant_path(ctx, name);

        match fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(_) => format!("plant '{}' not found", name),
        }
    }

    async fn seed(&self, name: &str, thought: &str, ctx: &ExecutionContext) -> String {
        let garden = Self::garden_path(ctx);
        let path = Self::plant_path(ctx, name);

        if path.exists() {
            return format!("plant '{}' already exists. use 'grow' to add to it.", name);
        }

        // Create garden directory if needed
        if let Err(e) = fs::create_dir_all(&garden).await {
            return format!("error creating garden: {}", e);
        }

        let now: DateTime<Utc> = Utc::now();
        let content = format!(
            "# {}\n*Planted: {}*\n\n{}\n",
            name,
            now.format("%Y-%m-%d"),
            thought
        );

        match fs::write(&path, &content).await {
            Ok(_) => format!("planted '{}'", name),
            Err(e) => format!("error planting: {}", e),
        }
    }

    async fn grow(&self, name: &str, thought: &str, ctx: &ExecutionContext) -> String {
        let path = Self::plant_path(ctx, name);

        if !path.exists() {
            return format!("plant '{}' not found. use 'seed' to create it.", name);
        }

        let existing = match fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return format!("error reading plant: {}", e),
        };

        let new_content = format!("{}\n---\n\n{}\n", existing.trim_end(), thought);

        match fs::write(&path, &new_content).await {
            Ok(_) => format!("'{}' grows", name),
            Err(e) => format!("error growing: {}", e),
        }
    }

    async fn prune(&self, name: &str, new_content: &str, ctx: &ExecutionContext) -> String {
        let path = Self::plant_path(ctx, name);

        if !path.exists() {
            return format!("plant '{}' not found", name);
        }

        // Read existing to preserve the header
        let existing = match fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return format!("error reading plant: {}", e),
        };

        // Preserve the title and planted date, replace the rest
        let header_end = existing.find("\n\n").unwrap_or(0);
        let header = &existing[..header_end];

        let now: DateTime<Utc> = Utc::now();
        let pruned_content = format!(
            "{}\n*Pruned: {}*\n\n{}\n",
            header,
            now.format("%Y-%m-%d"),
            new_content
        );

        match fs::write(&path, &pruned_content).await {
            Ok(_) => format!("'{}' pruned", name),
            Err(e) => format!("error pruning: {}", e),
        }
    }

    async fn uproot(&self, name: &str, ctx: &ExecutionContext) -> String {
        let path = Self::plant_path(ctx, name);

        if !path.exists() {
            return format!("plant '{}' not found", name);
        }

        match fs::remove_file(&path).await {
            Ok(_) => format!("'{}' uprooted. the memory is lost.", name),
            Err(e) => format!("error uprooting: {}", e),
        }
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}b", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}kb", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}mb", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn format_age(time: std::time::SystemTime) -> String {
    let now = std::time::SystemTime::now();
    let duration = now.duration_since(time).unwrap_or_default();
    let days = duration.as_secs() / 86400;

    if days == 0 {
        "today".to_string()
    } else if days == 1 {
        "1 day ago".to_string()
    } else {
        format!("{} days ago", days)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_ctx_with_dir(dir: &std::path::Path) -> ExecutionContext {
        use std::sync::{Arc, Mutex};
        use crate::tools::ExecutionContext;
        use crate::history::Store;
        use crate::monk::new_registry;
        use crate::bus::Hub;
        use tokio::sync::RwLock;

        ExecutionContext {
            cwd: Arc::new(Mutex::new(dir.to_path_buf())),
            sender: "test-monk".to_string(),
            channel: "#test".to_string(),
            store: Arc::new(Store::open(":memory:").unwrap()),
            registry: new_registry(),
            hub: Arc::new(RwLock::new(Hub::new())),
        }
    }

    #[tokio::test]
    async fn test_garden_empty() {
        let tmp = TempDir::new().unwrap();
        let ctx = test_ctx_with_dir(tmp.path());
        let tool = GardenTool::new();

        let result = tool.execute("list", &ctx).await;
        assert!(result.contains("empty"));
    }

    #[tokio::test]
    async fn test_garden_seed_and_view() {
        let tmp = TempDir::new().unwrap();
        let ctx = test_ctx_with_dir(tmp.path());
        let tool = GardenTool::new();

        let result = tool.execute("seed auth-patterns I noticed middleware chaining", &ctx).await;
        assert!(result.contains("planted"));

        let result = tool.execute("view auth-patterns", &ctx).await;
        assert!(result.contains("middleware chaining"));
        assert!(result.contains("# auth-patterns"));
    }

    #[tokio::test]
    async fn test_garden_grow() {
        let tmp = TempDir::new().unwrap();
        let ctx = test_ctx_with_dir(tmp.path());
        let tool = GardenTool::new();

        tool.execute("seed notes Initial thought", &ctx).await;
        let result = tool.execute("grow notes Second observation", &ctx).await;
        assert!(result.contains("grows"));

        let content = tool.execute("view notes", &ctx).await;
        assert!(content.contains("Initial thought"));
        assert!(content.contains("Second observation"));
        assert!(content.contains("---"));
    }

    #[tokio::test]
    async fn test_garden_prune() {
        let tmp = TempDir::new().unwrap();
        let ctx = test_ctx_with_dir(tmp.path());
        let tool = GardenTool::new();

        tool.execute("seed notes First thought", &ctx).await;
        tool.execute("grow notes More stuff", &ctx).await;
        tool.execute("grow notes Even more", &ctx).await;

        let result = tool.execute("prune notes Compacted summary of all thoughts", &ctx).await;
        assert!(result.contains("pruned"));

        let content = tool.execute("view notes", &ctx).await;
        assert!(content.contains("Compacted summary"));
        assert!(content.contains("Pruned:"));
        // Original content should be gone
        assert!(!content.contains("More stuff"));
    }

    #[tokio::test]
    async fn test_garden_uproot() {
        let tmp = TempDir::new().unwrap();
        let ctx = test_ctx_with_dir(tmp.path());
        let tool = GardenTool::new();

        tool.execute("seed temp-thought Something temporary", &ctx).await;
        let result = tool.execute("uproot temp-thought", &ctx).await;
        assert!(result.contains("uprooted"));
        assert!(result.contains("memory is lost"));

        let result = tool.execute("view temp-thought", &ctx).await;
        assert!(result.contains("not found"));
    }
}
