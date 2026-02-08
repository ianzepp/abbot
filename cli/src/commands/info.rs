//! Info command - Show system configuration, status, and health

use std::path::PathBuf;

use crate::config;
use crate::error::CliError;

pub async fn run(cli_config: Option<PathBuf>) -> Result<(), CliError> {
    use abbot::runtime::AppConfig;
    use abbot::runtime::app_config;

    config::load_api_keys();
    config::init_app_config(cli_config.as_deref());

    let config = AppConfig::global();
    let config_path = cli_config.or_else(config::default_config_path);
    let keys_path = app_config::keys_path();

    fn format_size(bytes: u64) -> String {
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        }
    }

    fn file_size(path: &std::path::Path) -> String {
        match std::fs::metadata(path) {
            Ok(m) => format_size(m.len()),
            Err(_) => "-".to_string(),
        }
    }

    fn file_preview(path: &std::path::Path, max_lines: usize) -> Option<String> {
        let content = std::fs::read_to_string(path).ok()?;
        let lines: Vec<&str> = content.lines().take(max_lines).collect();
        if lines.is_empty() {
            return None;
        }
        Some(lines.join("\n"))
    }

    fn check_mark(ok: bool) -> &'static str {
        if ok { "[x]" } else { "[ ]" }
    }

    // === Paths ===
    println!("# Abbot System Info\n");

    println!("## Paths\n");
    if let Some(ref p) = config_path {
        println!("- {} config: `{}`", check_mark(p.exists()), p.display());
    }
    if let Some(ref p) = keys_path {
        println!("- {} keys: `{}`", check_mark(p.exists()), p.display());
    }
    if let Some(ws) = app_config::config_dir() {
        println!("- {} data: `{}`", check_mark(ws.exists()), ws.display());
    } else {
        println!("- [ ] data: could not determine ~/.abbot/");
    }
    println!();

    // === Databases ===
    if let Some(data_dir) = app_config::config_dir()
        && data_dir.exists()
    {
        println!("## Databases\n");
        println!("| Database | Size | Purpose |");
        println!("|----------|------|---------|");
        println!(
            "| store.db | {} | conversations |",
            file_size(&data_dir.join("store.db"))
        );
        println!(
            "| ems.db | {} | entities |",
            file_size(&data_dir.join("ems.db"))
        );
        println!(
            "| frames.db | {} | frame history |",
            file_size(&data_dir.join("frames.db"))
        );
        println!();

        // === Memory ===
        let self_path = data_dir.join("mind").join("self.md");
        let memory_path = data_dir.join("mind").join("memory.md");

        if self_path.exists() || memory_path.exists() {
            println!("## Memory\n");

            if self_path.exists() {
                println!("**self.md** ({})", file_size(&self_path));
                if let Some(preview) = file_preview(&self_path, 4) {
                    println!("```");
                    println!("{}", preview);
                    println!("```");
                }
                println!();
            }

            if memory_path.exists() {
                println!("**memory.md** ({})", file_size(&memory_path));
                if let Some(preview) = file_preview(&memory_path, 4) {
                    println!("```");
                    println!("{}", preview);
                    println!("```");
                }
                println!();
            }
        }
    }

    // === Agents ===
    println!("## Agents\n");
    println!("| Agent | Model | Temp | Other |");
    println!("|-------|-------|------|-------|");

    let model = config.llm.model.as_deref().unwrap_or("-");
    let temp = config
        .llm
        .temperature
        .map(|t| t.to_string())
        .unwrap_or("-".into());

    println!(
        "| head | {} | {} | pool: {} |",
        model,
        temp,
        config.head.pool.unwrap_or(1),
    );
    println!(
        "| hand | {} | {} | pool: {} |",
        model,
        temp,
        config.hand.pool.unwrap_or(8),
    );
    println!(
        "| mind | {} | {} | pool: {} |",
        model,
        temp,
        config.mind.pool.unwrap_or(1),
    );
    println!();

    // === Provider Tests ===
    println!("## Providers\n");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| CliError::General(e.to_string()))?;

    let mut models_to_test: Vec<&str> = Vec::new();
    if let Some(ref m) = config.llm.model
        && !models_to_test.contains(&m.as_str())
    {
        models_to_test.push(m.as_str());
    }
    if let Some(ref m) = config.prompt_cache.llm.model
        && !models_to_test.contains(&m.as_str())
    {
        models_to_test.push(m.as_str());
    }

    let mut tested_providers: Vec<&str> = Vec::new();
    for model_id in &models_to_test {
        let provider = model_id.split('/').next().unwrap_or("");
        if provider.is_empty() || tested_providers.contains(&provider) {
            continue;
        }
        tested_providers.push(provider);

        let provider_config = config.providers.get(provider);
        let base_url = provider_config
            .and_then(|p| p.base_url.as_deref())
            .unwrap_or(match provider {
                "openrouter" => "https://openrouter.ai/api/v1",
                "anthropic" => "https://api.anthropic.com/v1",
                "openai" => "https://api.openai.com/v1",
                "ollama" => "http://localhost:11434/v1",
                _ => "",
            });

        let env_var = provider_config
            .and_then(|p| p.api_key_env.as_deref())
            .unwrap_or(match provider {
                "anthropic" => "ANTHROPIC_API_KEY",
                "openai" => "OPENAI_API_KEY",
                "openrouter" => "OPENROUTER_API_KEY",
                _ => "",
            });

        let api_key = if !env_var.is_empty() {
            std::env::var(env_var).ok()
        } else {
            None
        };

        let status = match provider {
            "openrouter" => {
                super::providers::test_openrouter(&client, base_url, api_key.as_deref()).await
            }
            "anthropic" => {
                super::providers::test_anthropic(&client, base_url, api_key.as_deref()).await
            }
            "openai" => super::providers::test_openai(&client, base_url, api_key.as_deref()).await,
            "ollama" => super::providers::test_ollama(&client, base_url).await,
            _ => "unknown provider".to_string(),
        };

        let icon = if status == "ok" { "[x]" } else { "[ ]" };
        if status == "ok" {
            println!("- {} **{}**: `{}`", icon, provider, base_url);
        } else {
            println!("- {} **{}**: `{}` - {}", icon, provider, base_url, status);
        }
    }

    if tested_providers.is_empty() {
        println!("- [ ] (no providers configured)");
    }

    println!();
    Ok(())
}
