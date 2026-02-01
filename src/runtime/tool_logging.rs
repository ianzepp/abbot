pub fn summarize_tool_args(tool: &str, args_json: &str) -> serde_json::Value {
    use serde_json::{json, Value};

    fn type_name(v: &Value) -> &'static str {
        match v {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }

    let Ok(v) = serde_json::from_str::<Value>(args_json) else {
        return json!({"keys": [], "raw_len": args_json.len(), "parse": "error"});
    };

    let Value::Object(map) = v else {
        return json!({"keys": [], "raw_type": type_name(&v)});
    };

    let mut out = serde_json::Map::new();
    let keys: Vec<String> = map.keys().cloned().collect();
    out.insert("keys".to_string(), json!(keys));

    // Tool-specific, conservative summaries.
    if tool == "bash" {
        if let Some(Value::String(cmd)) = map.get("command") {
            let looks_sensitive = {
                let lower = cmd.to_ascii_lowercase();
                lower.contains("api_key")
                    || lower.contains("token")
                    || lower.contains("password")
                    || lower.contains("authorization:")
                    || lower.contains("cookie:")
            };
            if looks_sensitive {
                out.insert("command_preview".to_string(), json!("<redacted>"));
                out.insert("redacted".to_string(), json!(true));
            } else {
                let preview: String = cmd.chars().take(120).collect();
                out.insert("command_preview".to_string(), json!(preview));
                out.insert("redacted".to_string(), json!(false));
            }
            out.insert("command_len".to_string(), json!(cmd.len()));
        }
        return Value::Object(out);
    }

    for (k, val) in map {
        let k_lower = k.to_ascii_lowercase();
        let is_sensitive_key = k_lower.contains("key")
            || k_lower.contains("token")
            || k_lower.contains("secret")
            || k_lower.contains("password")
            || k_lower.contains("authorization")
            || k_lower.contains("cookie");

        if is_sensitive_key {
            match val {
                Value::String(s) => out.insert(format!("{}_len", k), json!(s.len())),
                _ => out.insert(format!("{}_type", k), json!(type_name(&val))),
            };
            continue;
        }

        if matches!(k.as_str(), "content" | "patch" | "patchText") {
            if let Value::String(s) = val {
                out.insert(format!("{}_len", k), json!(s.len()));
            } else {
                out.insert(format!("{}_type", k), json!(type_name(&val)));
            }
            continue;
        }

        match val {
            Value::Bool(b) => {
                out.insert(k, json!(b));
            }
            Value::Number(n) => {
                out.insert(k, json!(n));
            }
            Value::String(s) => {
                let trimmed = s.trim();
                if trimmed.len() <= 160 {
                    out.insert(k, json!(trimmed));
                } else {
                    out.insert(format!("{}_len", k), json!(s.len()));
                }
            }
            Value::Array(a) => {
                out.insert(format!("{}_len", k), json!(a.len()));
            }
            Value::Object(o) => {
                out.insert(
                    format!("{}_keys", k),
                    json!(o.keys().cloned().collect::<Vec<_>>()),
                );
            }
            Value::Null => {
                out.insert(k, Value::Null);
            }
        }
    }

    Value::Object(out)
}
