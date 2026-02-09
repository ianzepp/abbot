use std::sync::Arc;

use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::hal::llm::{LlmClient, UnifiedMessage as LlmMessage};
use crate::history::Store;
use crate::runtime::AppConfig;

use super::session_scope::extract_env_block;

const MAX_LINES: usize = 200;

pub async fn process_user_system_prompt(
    store: Arc<Store>,
    room: &str,
    raw_prompt: &str,
    tool_names: &[String],
) -> Result<Option<String>, String> {
    let mut sanitized = raw_prompt.replace('\r', "");

    sanitized = strip_instructions_sections(&sanitized, &["AGENTS.md", "CLAUDE.md"]);

    while let Some(env_block) = extract_env_block(&sanitized) {
        sanitized = sanitized.replace(&env_block, "");
    }
    sanitized = strip_tag_blocks(&sanitized, "directories");

    let normalized = sanitized.trim();
    if normalized.is_empty() {
        return Ok(None);
    }

    let prompt_hash = hash_prompt(normalized);

    if let Some(cached) = store
        .get_cached_user_prompt(&prompt_hash)
        .await
        .map_err(|e| e.to_string())?
    {
        store
            .set_room_user_prompt(room, &prompt_hash)
            .await
            .map_err(|e| e.to_string())?;
        debug!(room, hash = %prompt_hash, "user system prompt cache hit");
        return Ok(Some(cached));
    }

    let summary = match generate_summary(normalized, tool_names).await {
        Ok(text) => text,
        Err(err) => {
            // If the minifier is unavailable (missing config, upstream down, etc.), do NOT cache a
            // clipped version of the user's prompt. This avoids persisting large/raw user prompts
            // when the intent is prompt-minification.
            debug!(room, error = %err, "prompt minifier unavailable; skipping user prompt cache");
            return Ok(None);
        }
    };

    if summary.trim().is_empty() {
        warn!(
            room,
            "prompt minifier returned empty output; skipping cache"
        );
        return Ok(None);
    }

    let rewritten = apply_tool_aliases(enforce_line_limit(&summary), tool_names)
        .trim()
        .to_string();

    if rewritten.is_empty() {
        warn!(
            room,
            "prompt minifier returned empty output; skipping cache"
        );
        return Ok(None);
    }

    store
        .put_cached_user_prompt(&prompt_hash, &rewritten)
        .await
        .map_err(|e| e.to_string())?;
    store
        .set_room_user_prompt(room, &prompt_hash)
        .await
        .map_err(|e| e.to_string())?;

    let line_count = rewritten.lines().count();
    info!(room, hash = %prompt_hash, lines = line_count, "user system prompt cached");
    Ok(Some(rewritten))
}

fn strip_instructions_sections(input: &str, filenames: &[&str]) -> String {
    if input.trim().is_empty() {
        return String::new();
    }

    let mut out: Vec<String> = Vec::new();
    let mut skipping = false;

    for line in input.lines() {
        if let Some(rest) = line.strip_prefix("Instructions from:") {
            let target = rest.trim();
            skipping = filenames.iter().any(|needle| target.contains(needle));
            if !skipping {
                out.push(line.to_string());
            }
            continue;
        }

        if skipping {
            continue;
        }

        out.push(line.to_string());
    }

    out.join("\n")
}

fn strip_tag_blocks(input: &str, tag: &str) -> String {
    let mut s = input.to_string();
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");

    loop {
        let Some(start) = s.find(&open) else {
            break;
        };
        let Some(rel_end) = s[start + open.len()..].find(&close) else {
            break;
        };
        let end = start + open.len() + rel_end + close.len();
        s.replace_range(start..end, "");
    }

    s
}

async fn generate_summary(content: &str, tool_names: &[String]) -> Result<String, String> {
    let client = build_prompt_minifier_client()
        .map_err(|e| format!("prompt minifier client init failed: {e}"))?;

    let mut system_prompt = include_str!("../prompts/head/compactor.md").to_string();

    if !tool_names.is_empty() {
        system_prompt.push_str(
            "\nTool routing: replace each user tool name with its middleware name as follows:\n",
        );
        for name in tool_names {
            system_prompt.push_str(&format!("- {0} => user__{0}\n", name));
        }
    }

    system_prompt.push_str(
        "\nReturn only the rewritten instructions. Do not add commentary about what changed.",
    );

    let user_content = format!(
        "<<<BEGIN USER PROMPT>>>\n{}\n<<<END USER PROMPT>>>",
        content
    );

    let response = client
        .chat(vec![
            LlmMessage::System(system_prompt),
            LlmMessage::User(user_content),
        ])
        .await
        .map_err(|e| format!("prompt minifier call failed: {e}"))?;

    Ok(response)
}

fn hash_prompt(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{:02x}", byte);
    }
    hex
}

fn enforce_line_limit(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
    }
    lines.join("\n").trim().to_string()
}

fn apply_tool_aliases(text: String, tool_names: &[String]) -> String {
    let mut out = text;
    for name in tool_names {
        if name.is_empty() {
            continue;
        }
        let alias = format!("user__{}", name);
        let mut start = 0;
        while let Some(idx) = out[start..].find(name) {
            let abs = start + idx;
            let end = abs + name.len();
            if abs >= 6 && &out[abs.saturating_sub(6)..abs] == "user__" {
                start = end;
                continue;
            }

            if !is_word_boundary(&out, abs, end) {
                start = end;
                continue;
            }

            out.replace_range(abs..end, &alias);
            start = abs + alias.len();
        }
    }
    out
}

fn is_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let bytes = text.as_bytes();
    let prev = start.checked_sub(1).and_then(|i| bytes.get(i)).copied();
    let next = bytes.get(end).copied();
    prev.map(|b| !is_ident_char(b)).unwrap_or(true)
        && next.map(|b| !is_ident_char(b)).unwrap_or(true)
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')
}

fn build_prompt_minifier_client() -> Result<LlmClient, String> {
    let app = AppConfig::global();
    let pc = &app.prompt_cache;
    if !pc.enabled.unwrap_or(false) {
        return Err("prompt minifier disabled".to_string());
    }

    let model_id = pc.llm.model.as_deref().unwrap_or("").trim();
    if model_id.is_empty() {
        return Err("prompt minifier model is not configured".to_string());
    }

    let temperature = pc.llm.temperature.or(Some(0.2));
    let max_tokens = pc.llm.max_tokens.or(Some(1200));
    LlmClient::from_model_id_with_options(model_id, temperature, max_tokens)
        .map_err(|e| e.to_string())
}
