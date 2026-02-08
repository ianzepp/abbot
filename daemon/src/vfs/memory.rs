//! MemoryFs - In-memory filesystem for VFS root
//!
//! Provides an ephemeral filesystem backed by a HashMap behind Arc<RwLock>.
//! Used as the VFS root so that unmatched paths resolve to memory instead of
//! being rejected. The LLM gets scratch space everywhere, but can only touch
//! real files through named host mounts.

use std::collections::HashMap;
use std::sync::Arc;

use globset::GlobMatcher;
use regex::Regex;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::kernel::KernelError;

// =============================================================================
// TYPES
// =============================================================================

/// A hit from searching memory file contents.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub path: String,
    pub line: usize,
    pub text: String,
}

/// An entry in the in-memory filesystem.
#[derive(Debug, Clone)]
enum MemoryEntry {
    File(Vec<u8>),
    Dir,
}

// =============================================================================
// MEMORYFS
// =============================================================================

/// In-memory filesystem backed by `HashMap<String, MemoryEntry>`.
///
/// Interior mutability via `Arc<RwLock>` makes this `Clone` (cheap Arc bump)
/// and safe for concurrent access across syscall executions.
#[derive(Debug, Clone)]
pub struct MemoryFs {
    entries: Arc<RwLock<HashMap<String, MemoryEntry>>>,
}

impl MemoryFs {
    /// Create a new MemoryFs with a root directory entry.
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        entries.insert("/".to_string(), MemoryEntry::Dir);
        Self {
            entries: Arc::new(RwLock::new(entries)),
        }
    }

    /// Read file content as UTF-8 (lossy).
    pub async fn read(&self, path: &str) -> Result<String, KernelError> {
        let entries = self.entries.read().await;
        match entries.get(path) {
            Some(MemoryEntry::File(data)) => Ok(String::from_utf8_lossy(data).to_string()),
            Some(MemoryEntry::Dir) => Err(KernelError::invalid_args(format!(
                "path is a directory, not a file: {path}"
            ))),
            None => Err(KernelError::not_found(format!("file not found: {path}"))),
        }
    }

    /// Write file content, auto-creating parent directories.
    pub async fn write(&self, path: &str, content: &[u8]) -> Result<(), KernelError> {
        let mut entries = self.entries.write().await;

        // Auto-create parent directories
        let mut current = String::new();
        if let Some(parent) = path.rsplit_once('/') {
            for component in parent.0.split('/') {
                if component.is_empty() {
                    current.push('/');
                    continue;
                }
                if !current.ends_with('/') {
                    current.push('/');
                }
                current.push_str(component);
                entries.entry(current.clone()).or_insert(MemoryEntry::Dir);
            }
        }

        entries.insert(path.to_string(), MemoryEntry::File(content.to_vec()));
        Ok(())
    }

    /// Create a directory entry. If `parents` is true, create all intermediate
    /// directories as well.
    pub async fn mkdir(&self, path: &str, parents: bool) -> Result<(), KernelError> {
        let mut entries = self.entries.write().await;

        if parents {
            let mut current = String::new();
            for component in path.split('/') {
                if component.is_empty() {
                    current.push('/');
                    continue;
                }
                if !current.ends_with('/') {
                    current.push('/');
                }
                current.push_str(component);
                entries.entry(current.clone()).or_insert(MemoryEntry::Dir);
            }
        } else {
            // Check parent exists
            if let Some((parent, _)) = path.rsplit_once('/') {
                let parent_key = if parent.is_empty() { "/" } else { parent };
                if !entries.contains_key(parent_key) {
                    return Err(KernelError::not_found(format!(
                        "parent directory does not exist: {parent_key}"
                    )));
                }
            }
            entries.insert(path.to_string(), MemoryEntry::Dir);
        }
        Ok(())
    }

    /// List immediate children of a directory.
    pub async fn list(&self, path: &str) -> Result<Vec<String>, KernelError> {
        let entries = self.entries.read().await;

        // Verify path is a directory
        match entries.get(path) {
            Some(MemoryEntry::Dir) => {}
            Some(MemoryEntry::File(_)) => {
                return Err(KernelError::invalid_args(format!(
                    "path is a file, not a directory: {path}"
                )));
            }
            None => {
                return Err(KernelError::not_found(format!(
                    "directory not found: {path}"
                )));
            }
        }

        let prefix = if path == "/" {
            "/".to_string()
        } else {
            format!("{path}/")
        };

        let mut children = Vec::new();
        for key in entries.keys() {
            if key == path {
                continue;
            }
            if let Some(suffix) = key.strip_prefix(&prefix) {
                // Only immediate children (no further slashes)
                if !suffix.contains('/') {
                    children.push(key.clone());
                }
            }
        }
        children.sort();
        Ok(children)
    }

    /// Check if a path exists.
    pub async fn exists(&self, path: &str) -> bool {
        self.entries.read().await.contains_key(path)
    }

    /// Check if a path is a directory.
    pub async fn is_dir(&self, path: &str) -> bool {
        matches!(self.entries.read().await.get(path), Some(MemoryEntry::Dir))
    }

    /// Remove a path (file or directory).
    pub async fn remove(&self, path: &str) -> Result<(), KernelError> {
        let mut entries = self.entries.write().await;
        if entries.remove(path).is_none() {
            return Err(KernelError::not_found(format!("path not found: {path}")));
        }
        Ok(())
    }

    /// Search file contents matching a regex, optionally filtered by glob on filename.
    pub async fn search(
        &self,
        dir: &str,
        pattern: &Regex,
        glob: Option<&GlobMatcher>,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, KernelError> {
        let entries = self.entries.read().await;

        let prefix = if dir == "/" {
            "/".to_string()
        } else {
            format!("{dir}/")
        };

        let mut hits = Vec::new();

        // Collect and sort keys for deterministic output
        let mut keys: Vec<&String> = entries.keys().collect();
        keys.sort();

        for key in keys {
            // Must be under the search directory
            if key != dir && !key.starts_with(&prefix) {
                continue;
            }

            let entry = &entries[key];
            let MemoryEntry::File(data) = entry else {
                continue;
            };

            // Apply glob filter on filename
            if let Some(glob) = glob {
                let filename = key.rsplit('/').next().unwrap_or(key);
                if !glob.is_match(filename) {
                    continue;
                }
            }

            let content = String::from_utf8_lossy(data);
            for (i, line) in content.lines().enumerate() {
                if pattern.is_match(line) {
                    let text: String = line.chars().take(400).collect();
                    hits.push(SearchHit {
                        path: key.clone(),
                        line: i + 1,
                        text,
                    });
                    if hits.len() >= max_results {
                        return Ok(hits);
                    }
                }
            }
        }

        Ok(hits)
    }
}

impl Default for MemoryFs {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_read_write() {
        let fs = MemoryFs::new();
        fs.write("/hello.txt", b"hello world").await.unwrap();
        let content = fs.read("/hello.txt").await.unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn test_read_missing_file() {
        let fs = MemoryFs::new();
        let err = fs.read("/nope.txt").await.unwrap_err();
        assert_eq!(err.code, "E_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_write_auto_creates_parents() {
        let fs = MemoryFs::new();
        fs.write("/a/b/c.txt", b"nested").await.unwrap();
        assert!(fs.is_dir("/a").await);
        assert!(fs.is_dir("/a/b").await);
        let content = fs.read("/a/b/c.txt").await.unwrap();
        assert_eq!(content, "nested");
    }

    #[tokio::test]
    async fn test_mkdir_parents() {
        let fs = MemoryFs::new();
        fs.mkdir("/x/y/z", true).await.unwrap();
        assert!(fs.is_dir("/x").await);
        assert!(fs.is_dir("/x/y").await);
        assert!(fs.is_dir("/x/y/z").await);
    }

    #[tokio::test]
    async fn test_mkdir_no_parents_fails() {
        let fs = MemoryFs::new();
        let err = fs.mkdir("/a/b", false).await.unwrap_err();
        assert_eq!(err.code, "E_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_list_root() {
        let fs = MemoryFs::new();
        fs.write("/foo.txt", b"foo").await.unwrap();
        fs.mkdir("/bar", true).await.unwrap();
        let children = fs.list("/").await.unwrap();
        assert!(children.contains(&"/bar".to_string()));
        assert!(children.contains(&"/foo.txt".to_string()));
    }

    #[tokio::test]
    async fn test_list_subdirectory() {
        let fs = MemoryFs::new();
        fs.write("/src/main.rs", b"fn main() {}").await.unwrap();
        fs.write("/src/lib.rs", b"// lib").await.unwrap();
        let children = fs.list("/src").await.unwrap();
        assert_eq!(children, vec!["/src/lib.rs", "/src/main.rs"]);
    }

    #[tokio::test]
    async fn test_list_missing_dir() {
        let fs = MemoryFs::new();
        let err = fs.list("/nope").await.unwrap_err();
        assert_eq!(err.code, "E_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_exists() {
        let fs = MemoryFs::new();
        assert!(fs.exists("/").await);
        assert!(!fs.exists("/nope").await);
        fs.write("/test.txt", b"test").await.unwrap();
        assert!(fs.exists("/test.txt").await);
    }

    #[tokio::test]
    async fn test_remove() {
        let fs = MemoryFs::new();
        fs.write("/rm_me.txt", b"bye").await.unwrap();
        assert!(fs.exists("/rm_me.txt").await);
        fs.remove("/rm_me.txt").await.unwrap();
        assert!(!fs.exists("/rm_me.txt").await);
    }

    #[tokio::test]
    async fn test_remove_missing() {
        let fs = MemoryFs::new();
        let err = fs.remove("/nope").await.unwrap_err();
        assert_eq!(err.code, "E_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_search() {
        let fs = MemoryFs::new();
        fs.write("/a.txt", b"hello world\nfoo bar\nhello again")
            .await
            .unwrap();
        fs.write("/b.txt", b"no match here").await.unwrap();

        let re = Regex::new("hello").unwrap();
        let hits = fs.search("/", &re, None, 100).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "/a.txt");
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[1].path, "/a.txt");
        assert_eq!(hits[1].line, 3);
    }

    #[tokio::test]
    async fn test_search_with_glob() {
        let fs = MemoryFs::new();
        fs.write("/src/main.rs", b"fn main() {}").await.unwrap();
        fs.write("/src/readme.md", b"fn not_rust()").await.unwrap();

        let re = Regex::new("fn").unwrap();
        let glob = globset::Glob::new("*.rs").unwrap().compile_matcher();
        let hits = fs.search("/src", &re, Some(&glob), 100).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "/src/main.rs");
    }

    #[tokio::test]
    async fn test_search_max_results() {
        let fs = MemoryFs::new();
        let content = "match\n".repeat(100);
        fs.write("/big.txt", content.as_bytes()).await.unwrap();

        let re = Regex::new("match").unwrap();
        let hits = fs.search("/", &re, None, 5).await.unwrap();
        assert_eq!(hits.len(), 5);
    }
}
