use super::Tool;
use tokio::fs;

pub struct EditTool;

#[async_trait::async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit file: !edit <file> s/old/new/ or !edit <file> append <text>"
    }

    async fn execute(&self, args: &str) -> String {
        let args = args.trim();
        if args.is_empty() {
            return "usage: !edit <file> s/old/new/ | !edit <file> append <text>".to_string();
        }

        let mut parts = args.splitn(2, ' ');
        let path = match parts.next() {
            Some(p) => p,
            None => return "error: no file specified".to_string(),
        };

        let operation = match parts.next() {
            Some(op) => op.trim(),
            None => return "error: no operation specified".to_string(),
        };

        if operation.starts_with("s/") {
            self.substitute(path, operation).await
        } else if operation.starts_with("append ") {
            let text = operation.strip_prefix("append ").unwrap();
            self.append(path, text).await
        } else {
            "error: unknown operation (use s/old/new/ or append <text>)".to_string()
        }
    }
}

impl EditTool {
    async fn substitute(&self, path: &str, pattern: &str) -> String {
        let parts: Vec<&str> = pattern.split('/').collect();
        if parts.len() < 4 {
            return "error: invalid substitution pattern (use s/old/new/)".to_string();
        }

        let old = parts[1];
        let new = parts[2];

        if old.is_empty() {
            return "error: empty search pattern".to_string();
        }

        let content = match fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => return format!("error reading file: {}", e),
        };

        if !content.contains(old) {
            return format!("error: '{}' not found in file", old);
        }

        let new_content = content.replacen(old, new, 1);

        match fs::write(path, &new_content).await {
            Ok(_) => format!("replaced '{}' with '{}'", old, new),
            Err(e) => format!("error writing file: {}", e),
        }
    }

    async fn append(&self, path: &str, text: &str) -> String {
        let mut content = match fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => return format!("error reading file: {}", e),
        };

        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(text);
        content.push('\n');

        match fs::write(path, &content).await {
            Ok(_) => "appended".to_string(),
            Err(e) => format!("error writing file: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_edit_empty() {
        let tool = EditTool;
        let result = tool.execute("").await;
        assert!(result.contains("usage"));
    }

    #[tokio::test]
    async fn test_edit_no_operation() {
        let tool = EditTool;
        let result = tool.execute("somefile.txt").await;
        assert!(result.contains("error"));
    }
}
