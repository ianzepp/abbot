use axum::http::HeaderMap;
use base64::Engine;
use sha2::Digest;

pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}

/// Check `x-api-key` header first, then fall back to `Authorization: Bearer`.
pub fn api_key_or_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .or_else(|| bearer_token(headers))
}

pub fn extract_env_block(content: &str) -> Option<String> {
    let start = content.find("<env>")?;
    let end = content.find("</env>")?;
    if end <= start {
        return None;
    }
    Some(content[start..end + 6].to_string())
}

pub fn extract_env_cwd(env_block: &str) -> Option<String> {
    for line in env_block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Working directory:") {
            let cwd = rest.trim();
            if !cwd.is_empty() {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

/// Multi-heuristic cwd extraction from system prompt text.
///
/// Checks in order:
/// 1. `<env>` block with "Working directory:" (OpenCode format)
/// 2. "Primary working directory:" line (Claude Code format)
/// 3. "working directory:" case-insensitive fallback
pub fn extract_cwd_heuristic(system_text: &str) -> Option<String> {
    // 1. OpenCode <env> block
    if let Some(env_block) = extract_env_block(system_text) {
        if let Some(cwd) = extract_env_cwd(&env_block) {
            return Some(cwd);
        }
    }

    // 2. "Primary working directory:" (Claude Code format)
    for line in system_text.lines() {
        let trimmed = line.trim().trim_start_matches("- ");
        if let Some(rest) = trimmed.strip_prefix("Primary working directory:") {
            let cwd = rest.trim();
            if !cwd.is_empty() {
                return Some(cwd.to_string());
            }
        }
    }

    // 3. Case-insensitive "working directory:" fallback
    for line in system_text.lines() {
        let lower = line.trim().to_ascii_lowercase();
        if let Some(idx) = lower.find("working directory:") {
            let rest = line.trim()[idx + "working directory:".len()..].trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }

    None
}

fn jwt_principal(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload_b64 = parts[1];
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    for key in ["sub", "email", "name"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn sha256_hex(s: &str) -> String {
    let mut h = sha2::Sha256::new();
    h.update(s.as_bytes());
    let out = h.finalize();
    let mut hex = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{:02x}", b);
    }
    hex
}

pub fn session_scope_from(token: &str, cwd: &str) -> String {
    let principal =
        jwt_principal(token).unwrap_or_else(|| format!("token:{}", &sha256_hex(token)[..16]));
    let digest = sha256_hex(&format!("{}:{}", principal, cwd));
    format!("session/{}", &digest[..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- bearer_token --

    #[test]
    fn bearer_token_extracts_value() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer sk-test-123".parse().unwrap());
        assert_eq!(bearer_token(&h), Some("sk-test-123"));
    }

    #[test]
    fn bearer_token_none_without_prefix() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Basic abc".parse().unwrap());
        assert_eq!(bearer_token(&h), None);
    }

    #[test]
    fn bearer_token_none_when_empty() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer   ".parse().unwrap());
        assert_eq!(bearer_token(&h), None);
    }

    // -- api_key_or_bearer --

    #[test]
    fn api_key_or_bearer_prefers_x_api_key() {
        let mut h = HeaderMap::new();
        h.insert("x-api-key", "sk-ant-key".parse().unwrap());
        h.insert("authorization", "Bearer sk-bearer".parse().unwrap());
        assert_eq!(api_key_or_bearer(&h), Some("sk-ant-key"));
    }

    #[test]
    fn api_key_or_bearer_falls_back_to_bearer() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer sk-bearer".parse().unwrap());
        assert_eq!(api_key_or_bearer(&h), Some("sk-bearer"));
    }

    #[test]
    fn api_key_or_bearer_none_when_both_missing() {
        let h = HeaderMap::new();
        assert_eq!(api_key_or_bearer(&h), None);
    }

    #[test]
    fn api_key_or_bearer_skips_empty_x_api_key() {
        let mut h = HeaderMap::new();
        h.insert("x-api-key", "  ".parse().unwrap());
        h.insert("authorization", "Bearer sk-bearer".parse().unwrap());
        assert_eq!(api_key_or_bearer(&h), Some("sk-bearer"));
    }

    // -- extract_env_block --

    #[test]
    fn extract_env_block_basic() {
        let input = "before\n<env>\nWorking directory: /tmp\n</env>\nafter";
        let block = extract_env_block(input).unwrap();
        assert!(block.contains("<env>"));
        assert!(block.contains("</env>"));
    }

    #[test]
    fn extract_env_block_none_when_missing() {
        assert!(extract_env_block("no env here").is_none());
    }

    // -- extract_env_cwd --

    #[test]
    fn extract_env_cwd_basic() {
        let block = "<env>\nWorking directory: /home/user/project\n</env>";
        assert_eq!(extract_env_cwd(block), Some("/home/user/project".to_string()));
    }

    #[test]
    fn extract_env_cwd_none_when_missing() {
        assert!(extract_env_cwd("<env>\nSomething else\n</env>").is_none());
    }

    // -- extract_cwd_heuristic --

    #[test]
    fn cwd_heuristic_opencode_env_block() {
        let text = "You are opencode\n<env>\nWorking directory: /oc/project\n</env>";
        assert_eq!(extract_cwd_heuristic(text), Some("/oc/project".to_string()));
    }

    #[test]
    fn cwd_heuristic_claude_code_primary() {
        let text = "You are Claude Code.\n - Primary working directory: /cc/project\nMore stuff";
        assert_eq!(extract_cwd_heuristic(text), Some("/cc/project".to_string()));
    }

    #[test]
    fn cwd_heuristic_claude_code_no_dash_prefix() {
        let text = "Primary working directory: /cc/project2";
        assert_eq!(extract_cwd_heuristic(text), Some("/cc/project2".to_string()));
    }

    #[test]
    fn cwd_heuristic_case_insensitive_fallback() {
        let text = "The WORKING DIRECTORY: /fallback/path\n";
        assert_eq!(extract_cwd_heuristic(text), Some("/fallback/path".to_string()));
    }

    #[test]
    fn cwd_heuristic_prefers_env_over_primary() {
        let text = "<env>\nWorking directory: /env/path\n</env>\n - Primary working directory: /primary/path";
        assert_eq!(extract_cwd_heuristic(text), Some("/env/path".to_string()));
    }

    #[test]
    fn cwd_heuristic_none_when_no_match() {
        assert!(extract_cwd_heuristic("nothing relevant here").is_none());
    }

    // -- session_scope_from --

    #[test]
    fn session_scope_deterministic() {
        let a = session_scope_from("tok1", "/cwd");
        let b = session_scope_from("tok1", "/cwd");
        assert_eq!(a, b);
        assert!(a.starts_with("session/"));
    }

    #[test]
    fn session_scope_varies_by_cwd() {
        let a = session_scope_from("tok1", "/cwd1");
        let b = session_scope_from("tok1", "/cwd2");
        assert_ne!(a, b);
    }

    #[test]
    fn session_scope_varies_by_token() {
        let a = session_scope_from("tok1", "/cwd");
        let b = session_scope_from("tok2", "/cwd");
        assert_ne!(a, b);
    }
}
