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
