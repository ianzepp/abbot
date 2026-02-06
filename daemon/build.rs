use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let plugins_dir = Path::new(&manifest_dir).join("src").join("plugins");

    println!("cargo:rerun-if-changed={}", plugins_dir.display());
    emit_rerun_if_changed_recursive(&plugins_dir);

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let out_path = out_dir.join("builtin_plugins.rs");

    let entries = match fs::read_dir(&plugins_dir) {
        Ok(e) => e,
        Err(_) => {
            fs::write(&out_path, "").expect("write builtin_plugins.rs");
            return;
        }
    };

    let mut plugins: Vec<(String, String)> = Vec::new();

    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        let plugin_toml = entry.path().join("plugin.toml");
        if !plugin_toml.exists() {
            continue;
        }

        let expected_id = read_plugin_id(&plugin_toml).unwrap_or_else(|| dir_name.clone());

        let plugin_dir_lit = rust_string_literal(&dir_name);
        let expected_id_lit = rust_string_literal(&expected_id);

        let hand_md_path = entry.path().join("hand.md");
        let head_md_path = entry.path().join("head.md");

        let hand_expr = if hand_md_path.exists() {
            format!(
                "include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/src/plugins/{}/hand.md\"))",
                plugin_dir_lit
            )
        } else {
            "\"\"".to_string()
        };

        let head_expr = if head_md_path.exists() {
            format!(
                "include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/src/plugins/{}/head.md\"))",
                plugin_dir_lit
            )
        } else {
            "\"\"".to_string()
        };

        let stmt = format!(
            "add_builtin(\n    &mut out,\n    \"{}\",\n    include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/src/plugins/{}/plugin.toml\")),\n    {},\n    {},\n);\n",
            expected_id_lit, plugin_dir_lit, hand_expr, head_expr
        );

        plugins.push((expected_id, stmt));
    }

    plugins.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out_src = String::new();
    out_src.push_str("{\n");
    for (_, stmt) in plugins {
        out_src.push_str(&stmt);
        out_src.push('\n');
    }
    out_src.push_str("}\n");

    fs::write(&out_path, out_src).expect("write builtin_plugins.rs");
}

fn emit_rerun_if_changed_recursive(root: &Path) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if let Ok(ft) = entry.file_type() {
            if ft.is_dir() {
                emit_rerun_if_changed_recursive(&path);
                continue;
            }
        }
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn read_plugin_id(plugin_toml: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(plugin_toml).ok()?;
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != "id" {
            continue;
        }
        let v = v.trim();
        return strip_quotes(v);
    }
    None
}

fn strip_quotes(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        if (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
        {
            return Some(s[1..s.len() - 1].to_string());
        }
    }
    None
}

fn rust_string_literal(s: &str) -> String {
    // Minimal escaping for embedding inside a Rust string literal.
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}
