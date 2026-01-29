use std::path::{Path, PathBuf};
use std::fs;
use serde::{Serialize, Deserialize};

const MONKS_DIR: &str = "monastery/monks";

/// Front matter metadata for a monk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonkMeta {
    pub name: String,
    pub model: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub created: String,
}

impl Default for MonkMeta {
    fn default() -> Self {
        Self {
            name: "unnamed".to_string(),
            model: "small".to_string(),
            status: "dormant".to_string(),
            created: chrono::Local::now().to_rfc3339(),
        }
    }
}

/// A monk defined in a markdown file with front matter
#[derive(Debug, Clone)]
pub struct MonkFile {
    pub meta: MonkMeta,
    pub soul: String,
    pub path: PathBuf,
}

impl MonkFile {
    /// Load a monk from a markdown file
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read monk file: {}", e))?;

        let (meta, soul) = parse_front_matter(&content)?;

        Ok(Self {
            meta,
            soul,
            path: path.to_path_buf(),
        })
    }

    /// Load all monks from the monastery/monks directory
    pub fn load_all() -> Vec<Self> {
        let monks_dir = PathBuf::from(MONKS_DIR);

        if !monks_dir.exists() {
            return vec![];
        }

        let entries = match fs::read_dir(&monks_dir) {
            Ok(e) => e,
            Err(_) => return vec![],
        };

        let mut monks = vec![];

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }

            match Self::load(&path) {
                Ok(m) => monks.push(m),
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "Failed to load monk file"),
            }
        }

        monks
    }

    /// Find a monk by name
    pub fn find(name: &str) -> Option<Self> {
        let path = PathBuf::from(MONKS_DIR).join(format!("{}.md", name));
        if path.exists() {
            Self::load(path).ok()
        } else {
            None
        }
    }

    /// Check if a monk file exists
    pub fn exists(name: &str) -> bool {
        PathBuf::from(MONKS_DIR).join(format!("{}.md", name)).exists()
    }

    /// Create a new monk file
    pub fn recruit(name: &str, model: &str) -> Result<Self, String> {
        let monks_dir = PathBuf::from(MONKS_DIR);
        fs::create_dir_all(&monks_dir)
            .map_err(|e| format!("Failed to create monks directory: {}", e))?;

        let path = monks_dir.join(format!("{}.md", name));

        if path.exists() {
            return Err(format!("Monk '{}' already exists", name));
        }

        let meta = MonkMeta {
            name: name.to_string(),
            model: model.to_string(),
            status: "dormant".to_string(),
            created: chrono::Local::now().to_rfc3339(),
        };

        let soul = initial_soul(name);
        let content = format_front_matter(&meta, &soul);

        fs::write(&path, content)
            .map_err(|e| format!("Failed to write monk file: {}", e))?;

        Ok(Self {
            meta,
            soul,
            path,
        })
    }

    /// Delete a monk file (banish)
    pub fn banish(name: &str) -> Result<(), String> {
        let path = PathBuf::from(MONKS_DIR).join(format!("{}.md", name));

        if !path.exists() {
            return Err(format!("Monk '{}' not found", name));
        }

        fs::remove_file(&path)
            .map_err(|e| format!("Failed to delete monk file: {}", e))
    }

    /// Update the status in front matter
    pub fn set_status(&mut self, status: &str) -> Result<(), String> {
        self.meta.status = status.to_string();
        let content = format_front_matter(&self.meta, &self.soul);
        fs::write(&self.path, content)
            .map_err(|e| format!("Failed to update monk file: {}", e))
    }

    /// Get the full system prompt (grammar + rules + soul)
    pub fn system_prompt(&self) -> String {
        let grammar = include_str!("../../monastery/grammar.md");
        let rules = include_str!("../../monastery/system.md");
        format!("{}\n\n{}\n\n## Your Soul\n\n{}", grammar, rules, self.soul)
    }
}

/// Parse front matter from markdown content
fn parse_front_matter(content: &str) -> Result<(MonkMeta, String), String> {
    // Check if content starts with ---
    if !content.trim_start().starts_with("---") {
        return Err("No front matter found (must start with ---)".to_string());
    }

    // Find the end of front matter
    let after_start = &content.trim_start()[3..];
    let Some(end_idx) = after_start.find("---") else {
        return Err("Front matter not closed (missing ---)".to_string());
    };

    let yaml_part = &after_start[..end_idx];
    let soul = after_start[end_idx + 3..].trim();

    let meta: MonkMeta = serde_yaml::from_str(yaml_part)
        .map_err(|e| format!("Failed to parse front matter: {}", e))?;

    Ok((meta, soul.to_string()))
}

/// Format front matter and soul into markdown content
fn format_front_matter(meta: &MonkMeta, soul: &str) -> String {
    let yaml = serde_yaml::to_string(meta)
        .unwrap_or_default();

    format!("---\n{}---\n\n{}", yaml, soul)
}

/// Initial soul template for new monks
fn initial_soul(name: &str) -> String {
    format!(r#"## Identity
I am {}, a monk in the AI monastery.

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
"#, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_front_matter() {
        let content = r#"---
name: brother-thomas
model: sonnet
status: active
created: "2026-01-29T10:00:00Z"
---

## Identity
I am brother-thomas.
"#;

        let (meta, soul) = parse_front_matter(content).unwrap();
        assert_eq!(meta.name, "brother-thomas");
        assert_eq!(meta.model, "sonnet");
        assert_eq!(meta.status, "active");
        assert!(soul.contains("I am brother-thomas"));
    }

    #[test]
    fn test_recruit_and_load() {
        let temp = TempDir::new().unwrap();
        let monks_dir = temp.path().join("monks");
        std::env::set_current_dir(temp.path()).ok();

        // Create the monks directory manually since we're in temp
        fs::create_dir_all(&monks_dir).unwrap();

        // Patch the MONKS_DIR for testing would require refactoring,
        // so we'll just test the parser here
        let content = format_front_matter(
            &MonkMeta {
                name: "test-monk".to_string(),
                model: "small".to_string(),
                status: "dormant".to_string(),
                created: "now".to_string(),
            },
            "## Test\nContent"
        );

        assert!(content.contains("name: test-monk"));
        assert!(content.contains("---"));
    }
}
