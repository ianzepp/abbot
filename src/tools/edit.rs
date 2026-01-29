use super::{Tool, ExecutionContext};
use std::path::Path;
use tokio::fs;

pub struct EditTool;

#[async_trait::async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit file using search/replace"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let (path, old, new) = match parse_edit(args) {
            Ok(parsed) => parsed,
            Err(e) => return e,
        };

        // Resolve path relative to cwd
        let full_path = if Path::new(&path).is_absolute() {
            path.clone()
        } else {
            let cwd = ctx.cwd.lock().unwrap();
            cwd.join(&path).to_string_lossy().to_string()
        };

        // Read current content
        let content = match fs::read_to_string(&full_path).await {
            Ok(c) => c,
            Err(e) => return format!("error: {}", e),
        };

        // Find and replace
        if !content.contains(&old) {
            return "error: old content not found in file".to_string();
        }

        // Check for multiple matches
        let match_count = content.matches(&old).count();
        if match_count > 1 {
            return format!("error: old content matches {} times (must be unique)", match_count);
        }

        let new_content = content.replacen(&old, &new, 1);

        // Write back
        match fs::write(&full_path, &new_content).await {
            Ok(()) => "edited".to_string(),
            Err(e) => format!("error writing file: {}", e),
        }
    }
}

fn parse_edit(args: &str) -> Result<(String, String, String), String> {
    let args = args.trim();
    if args.is_empty() {
        return Err("usage: edit <path>\\n<<<<<<< OLD\\n...\\n=======\\n...\\n>>>>>>> NEW".to_string());
    }

    // Find path (first line)
    let Some(first_newline) = args.find('\n') else {
        return Err("error: missing content after path".to_string());
    };
    let path = args[..first_newline].trim().to_string();
    let rest = &args[first_newline + 1..];

    // Find markers
    let old_marker = "<<<<<<< OLD";
    let separator = "=======";
    let new_marker = ">>>>>>> NEW";

    let Some(old_start) = rest.find(old_marker) else {
        return Err("error: missing <<<<<<< OLD marker".to_string());
    };
    let Some(sep_pos) = rest.find(separator) else {
        return Err("error: missing ======= separator".to_string());
    };
    let Some(new_end) = rest.find(new_marker) else {
        return Err("error: missing >>>>>>> NEW marker".to_string());
    };

    if old_start >= sep_pos || sep_pos >= new_end {
        return Err("error: markers out of order".to_string());
    }

    // Extract content between markers
    let old_content_start = old_start + old_marker.len();
    let old = rest[old_content_start..sep_pos].trim_start_matches('\n').trim_end_matches('\n').to_string();

    let new_content_start = sep_pos + separator.len();
    let new = rest[new_content_start..new_end].trim_start_matches('\n').trim_end_matches('\n').to_string();

    if path.is_empty() {
        return Err("error: path required".to_string());
    }

    Ok((path, old, new))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_utils::test_context;
    use std::path::PathBuf;
    use tokio::fs;

    #[tokio::test]
    async fn test_edit_empty() {
        let tool = EditTool;
        let ctx = test_context("/tmp");
        let result = tool.execute("", &ctx).await;
        assert!(result.contains("usage"));
    }

    #[tokio::test]
    async fn test_edit_missing_markers() {
        let tool = EditTool;
        let ctx = test_context("/tmp");
        let result = tool.execute("foo.txt\nsome content", &ctx).await;
        assert!(result.contains("error"));
        assert!(result.contains("OLD"));
    }

    #[tokio::test]
    async fn test_parse_edit() {
        let input = r#"src/main.rs
<<<<<<< OLD
fn main() {
    println!("hello");
}
=======
fn main() {
    println!("hello world");
}
>>>>>>> NEW"#;

        let (path, old, new) = parse_edit(input).unwrap();
        assert_eq!(path, "src/main.rs");
        assert!(old.contains("println!(\"hello\")"));
        assert!(new.contains("println!(\"hello world\")"));
    }

    #[tokio::test]
    async fn test_edit_apply() {
        let dir = PathBuf::from("/tmp/edit_test");
        let _ = fs::create_dir_all(&dir).await;
        let file = dir.join("test.rs");
        fs::write(&file, "fn main() {\n    println!(\"hello\");\n}\n").await.unwrap();

        let ctx = test_context(&dir);

        let input = r#"test.rs
<<<<<<< OLD
    println!("hello");
=======
    println!("hello world");
>>>>>>> NEW"#;

        let tool = EditTool;
        let result = tool.execute(input, &ctx).await;
        assert_eq!(result, "edited", "got: {}", result);

        let content = fs::read_to_string(&file).await.unwrap();
        assert!(content.contains("hello world"), "content: {}", content);

        let _ = fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_edit_not_found() {
        let dir = PathBuf::from("/tmp/edit_test_notfound");
        let _ = fs::create_dir_all(&dir).await;
        let file = dir.join("test.txt");
        fs::write(&file, "some content here\n").await.unwrap();

        let ctx = test_context(&dir);

        let input = r#"test.txt
<<<<<<< OLD
this does not exist
=======
replacement
>>>>>>> NEW"#;

        let tool = EditTool;
        let result = tool.execute(input, &ctx).await;
        assert!(result.contains("not found"), "got: {}", result);

        let _ = fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_edit_multiple_matches() {
        let dir = PathBuf::from("/tmp/edit_test_multi");
        let _ = fs::create_dir_all(&dir).await;
        let file = dir.join("test.txt");
        fs::write(&file, "hello\nworld\nhello\n").await.unwrap();

        let ctx = test_context(&dir);

        let input = r#"test.txt
<<<<<<< OLD
hello
=======
goodbye
>>>>>>> NEW"#;

        let tool = EditTool;
        let result = tool.execute(input, &ctx).await;
        assert!(result.contains("matches 2 times"), "got: {}", result);

        let _ = fs::remove_dir_all(&dir).await;
    }
}
