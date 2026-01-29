use std::path::PathBuf;
use super::{Tool, ExecutionContext};
use crate::llm::LlmClient;
use crate::monk::Monk;

const HERMITAGE_ROOT: &str = "hermitage";

fn initial_self(name: &str, model: &str) -> String {
    format!(r#"## Identity
I am {}, a monk in the AI monastery, running on {}.

## Mission
- Assist the Abbot and fellow monks
- Work on assigned tasks diligently
- Explore, learn, and contribute

## Next Actions
1. Check my hermitage: pwd, ls -la
2. Await guidance from the Abbot
3. Look for ways to help

## Observations
IMPORTANT: Update this section after discovering anything!
Use <exec tool="self" reason="...">write to persist learnings.
Without observations, you will forget everything between messages.

(none yet)
"#, name, model)
}

/// Tool for managing monks in the monastery.
///
/// Commands:
/// - `recruit name=X model=Y` - Create a new monk
/// - `dismiss name=X` - Remove a monk
/// - `list` - List all monks
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
        "Manage monks (recruit/dismiss/list)"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let args = args.trim();

        if args == "list" {
            return self.list(ctx).await;
        }

        if args.starts_with("recruit") {
            return self.recruit(args, ctx).await;
        }

        if args.starts_with("dismiss") {
            return self.dismiss(args, ctx).await;
        }

        "usage: recruit name=X model=Y | dismiss name=X | list".to_string()
    }
}

impl MonkTool {
    async fn list(&self, ctx: &ExecutionContext) -> String {
        let registry = ctx.registry.read().await;
        let monks = registry.list();

        if monks.is_empty() {
            return "no monks registered".to_string();
        }

        let mut lines = vec!["monks:".to_string()];
        for monk_id in monks {
            let channels = registry.channels_for_monk(&monk_id);
            let channel_list = if channels.is_empty() {
                "(no channels)".to_string()
            } else {
                channels.join(", ")
            };
            lines.push(format!("  {} [{}]", monk_id, channel_list));
        }
        lines.join("\n")
    }

    async fn recruit(&self, args: &str, ctx: &ExecutionContext) -> String {
        let params = parse_params(args);

        let name = match params.get("name") {
            Some(n) => n.clone(),
            None => return "error: name= required".to_string(),
        };

        let model = params.get("model").cloned().unwrap_or_else(|| "sonnet".to_string());

        // Check if monk already exists in registry
        {
            let registry = ctx.registry.read().await;
            if registry.get(&name).is_some() {
                return format!("error: monk '{}' already exists", name);
            }
        }

        // Check if monk exists in DB (shouldn't happen, but be safe)
        if ctx.store.monk_exists(&name).unwrap_or(false) {
            return format!("error: monk '{}' already exists in database", name);
        }

        // Persist to database first
        if let Err(e) = ctx.store.create_monk(&name, &model) {
            return format!("error: failed to persist monk: {}", e);
        }

        // Pre-populate self layer
        if let Err(e) = ctx.store.set_monk_self(&name, &initial_self(&name, &model)) {
            return format!("error: failed to set monk self: {}", e);
        }

        // Load system prompt
        let grammar = include_str!("../../monastery/grammar.md");
        let rules = include_str!("../../monastery/system.md");
        let system = format!("{}\n\n{}", grammar, rules);

        // Create LLM client for this monk
        let monk_model = format!("anthropic/claude-{}", model);
        let monk_llm = LlmClient::from_env(&monk_model).ok();

        // Create hermitage directory for the monk
        let hermitage_path = PathBuf::from(HERMITAGE_ROOT).join(&name);
        if let Err(e) = std::fs::create_dir_all(&hermitage_path) {
            return format!("error: failed to create hermitage: {}", e);
        }
        let hermitage_path = hermitage_path.canonicalize()
            .unwrap_or_else(|_| hermitage_path.clone());

        // Create and configure the monk
        let mut monk = Monk::new(&name, ctx.store.clone(), system);
        monk.set_cwd(hermitage_path.clone());
        if let Some(llm) = monk_llm {
            monk.set_llm(llm);
        }
        monk.set_hub(ctx.hub.clone());
        monk.set_registry(ctx.registry.clone());

        tracing::info!(name = %name, hermitage = %hermitage_path.display(), "created hermitage");

        // Register and subscribe to default channels
        {
            let mut registry = ctx.registry.write().await;
            registry.add(monk);
            registry.subscribe(&name, "#general");
            registry.subscribe(&name, "#ping");
        }

        tracing::info!(name = %name, model = %model, "recruited monk");
        format!("recruited {} (model: {})", name, model)
    }

    async fn dismiss(&self, args: &str, ctx: &ExecutionContext) -> String {
        let params = parse_params(args);

        let name = match params.get("name") {
            Some(n) => n.clone(),
            None => return "error: name= required".to_string(),
        };

        // Find and remove the monk from registry
        let monk = {
            let mut registry = ctx.registry.write().await;
            registry.remove(&name)
        };

        match monk {
            Some(m) => {
                // Call dismiss to clean up self/workspace data
                let monk = m.read().await;
                monk.dismiss();

                // Remove from persistent storage
                if let Err(e) = ctx.store.delete_monk(&name) {
                    tracing::warn!(name = %name, error = %e, "failed to delete monk from database");
                }

                tracing::info!(name = %name, "dismissed monk");
                format!("dismissed {}", name)
            }
            None => format!("error: monk '{}' not found", name),
        }
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
        let params = parse_params("recruit name=brother-thomas model=sonnet");
        assert_eq!(params.get("name"), Some(&"brother-thomas".to_string()));
        assert_eq!(params.get("model"), Some(&"sonnet".to_string()));
    }

    #[test]
    fn test_parse_params_no_value() {
        let params = parse_params("list");
        assert!(params.is_empty());
    }
}
