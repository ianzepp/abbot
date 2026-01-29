use std::path::PathBuf;
use super::{Tool, ExecutionContext};
use crate::llm::{LlmClient, resolve_model};
use crate::monk::{Monk, MonkFile};

const HERMITAGE_ROOT: &str = "hermitage";

/// Tool for managing monks in the monastery.
///
/// Commands:
/// - `recruit name=X model=Y` - Create a new monk markdown file
/// - `banish name=X` - Delete a monk file (requires force=true)
/// - `wake name=X` - Instantiate a monk from file into the running server
/// - `dismiss name=X` - De-instantiate monk but keep file
/// - `list` - List all monk files and their status
pub struct MonkTool;

impl MonkTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Tool for MonkTool {
    fn name(&self) -> &str {
        "monk"
    }

    fn description(&self) -> &str {
        "Manage monks (recruit/banish/wake/dismiss/list)"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let args = args.trim();

        if args == "list" {
            return self.list(ctx).await;
        }

        if args.starts_with("recruit") {
            return self.recruit(args).await;
        }

        if args.starts_with("banish") {
            return self.banish(args).await;
        }

        if args.starts_with("wake") {
            return self.wake(args, ctx).await;
        }

        if args.starts_with("dismiss") {
            return self.dismiss(args, ctx).await;
        }

        "usage: recruit name=X model=Y | banish name=X force=true | wake name=X | dismiss name=X | list".to_string()
    }
}

impl MonkTool {
    /// List all monk files and their status
    async fn list(&self, ctx: &ExecutionContext) -> String {
        let files = MonkFile::load_all();
        let registry = ctx.registry.read().await;

        if files.is_empty() {
            return "no monks found in monastery/monks/".to_string();
        }

        let mut lines = vec!["monks:".to_string()];

        for file in files {
            let is_awake = registry.get(&file.meta.name).is_some();
            let status = if is_awake {
                "awake"
            } else {
                &file.meta.status
            };

            let channels = if is_awake {
                let ch = registry.channels_for_monk(&file.meta.name);
                if ch.is_empty() {
                    "(no channels)".to_string()
                } else {
                    ch.join(", ")
                }
            } else {
                "(dormant)".to_string()
            };

            lines.push(format!(
                "  {} [model: {}, status: {}] - {}",
                file.meta.name, file.meta.model, status, channels
            ));
        }

        lines.join("\n")
    }

    /// Create a new monk file (does not wake)
    async fn recruit(&self, args: &str) -> String {
        let params = parse_params(args);

        let name = match params.get("name") {
            Some(n) => n.clone(),
            None => return "error: name= required".to_string(),
        };

        let model = params.get("model").cloned().unwrap_or_else(|| "small".to_string());

        // Validate model
        let valid_models = ["small", "medium", "large"];
        if !valid_models.contains(&model.as_str()) {
            return format!("error: model must be one of: {:?}", valid_models);
        }

        // Create the monk file
        match MonkFile::recruit(&name, &model) {
            Ok(file) => {
                tracing::info!(name = %name, model = %model, path = %file.path.display(), "recruited monk");
                format!("recruited {} (model: {}) -> {}", name, model, file.path.display())
            }
            Err(e) => format!("error: {}", e),
        }
    }

    /// Delete a monk file (banish)
    async fn banish(&self, args: &str) -> String {
        let params = parse_params(args);

        let name = match params.get("name") {
            Some(n) => n.clone(),
            None => return "error: name= required".to_string(),
        };

        // Check for force=true
        let force = params.get("force").map(|v| v == "true").unwrap_or(false);

        if !force {
            return format!(
                "WARNING: Banishing {} will permanently delete their file.\n\
                 To confirm, run: banish name={} force=true",
                name, name
            );
        }

        match MonkFile::banish(&name) {
            Ok(_) => {
                tracing::info!(name = %name, "banished monk");
                format!("banished {} (file deleted)", name)
            }
            Err(e) => format!("error: {}", e),
        }
    }

    /// Instantiate a monk from file into running server
    async fn wake(&self, args: &str, ctx: &ExecutionContext) -> String {
        let params = parse_params(args);

        let name = match params.get("name") {
            Some(n) => n.clone(),
            None => return "error: name= required".to_string(),
        };

        // Check if already awake
        {
            let registry = ctx.registry.read().await;
            if registry.get(&name).is_some() {
                return format!("{} is already awake", name);
            }
        }

        // Load the monk file
        let file = match MonkFile::find(&name) {
            Some(f) => f,
            None => return format!("error: monk '{}' not found (try: recruit name={})", name, name),
        };

        // Create hermitage directory
        let hermitage_path = PathBuf::from(HERMITAGE_ROOT).join(&name);
        if let Err(e) = std::fs::create_dir_all(&hermitage_path) {
            return format!("error: failed to create hermitage: {}", e);
        }
        let hermitage_path = hermitage_path.canonicalize()
            .unwrap_or_else(|_| hermitage_path.clone());

        // Create LLM client
        let monk_model = resolve_model(&file.meta.model);
        let monk_llm = LlmClient::from_env(monk_model).ok();

        // Build system prompt from soul
        let system = file.system_prompt();

        // Create and configure monk
        let mut monk = Monk::new(&name, ctx.store.clone(), system);
        monk.set_cwd(hermitage_path.clone());
        if let Some(llm) = monk_llm {
            monk.set_llm(llm);
        }
        monk.set_hub(ctx.hub.clone());
        monk.set_registry(ctx.registry.clone());

        // Create cell channel
        let cell = format!("#cell-{}", name);
        ctx.hub.write().await.create_channel(&cell);

        // Register and subscribe
        {
            let mut registry = ctx.registry.write().await;
            registry.add(monk);
            registry.subscribe(&name, &cell);
            registry.subscribe(&name, "#ping");
            // Also subscribe the waking monk (sender) to the cell
            registry.subscribe(&ctx.sender, &cell);
        }

        // Update status in file
        let mut file = file;
        if let Err(e) = file.set_status("awake") {
            tracing::warn!(name = %name, error = %e, "failed to update monk status");
        }

        // Ensure monk exists in database for state storage
        if let Err(e) = ctx.store.create_monk(&name, &file.meta.model) {
            tracing::warn!(name = %name, error = %e, "failed to create monk in database");
        }

        tracing::info!(name = %name, hermitage = %hermitage_path.display(), cell = %cell, "woke monk");
        format!("woke {} (model: {}, cell: {})", name, file.meta.model, cell)
    }

    /// De-instantiate a monk but keep the file
    async fn dismiss(&self, args: &str, ctx: &ExecutionContext) -> String {
        let params = parse_params(args);

        let name = match params.get("name") {
            Some(n) => n.clone(),
            None => return "error: name= required".to_string(),
        };

        // Check if awake
        let was_awake = {
            let registry = ctx.registry.read().await;
            registry.get(&name).is_some()
        };

        if !was_awake {
            return format!("{} is already dormant (not awake)", name);
        }

        // Remove from registry
        let monk = {
            let mut registry = ctx.registry.write().await;
            registry.remove(&name)
        };

        // Clean up any runtime resources
        if let Some(m) = monk {
            let monk = m.read().await;
            monk.dismiss();
        }

        // Update status in file
        if let Some(mut file) = MonkFile::find(&name) {
            if let Err(e) = file.set_status("dormant") {
                tracing::warn!(name = %name, error = %e, "failed to update monk status");
            }
        }

        tracing::info!(name = %name, "dismissed monk (file preserved)");
        format!("dismissed {} (returned to dormancy, file preserved)", name)
    }
}

fn parse_params(args: &str) -> std::collections::HashMap<String, String> {
    let mut params = std::collections::HashMap::new();

    for part in args.split_whitespace() {
        if let Some((key, value)) = part.split_once('=') {
            params.insert(key.to_string(), value.to_string());
        }
    }

    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_params() {
        let params = parse_params("recruit name=brother-thomas model=small");
        assert_eq!(params.get("name"), Some(&"brother-thomas".to_string()));
        assert_eq!(params.get("model"), Some(&"small".to_string()));
    }

    #[test]
    fn test_parse_params_no_value() {
        let params = parse_params("list");
        assert!(params.is_empty());
    }
}
