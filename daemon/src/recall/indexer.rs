use crate::recall::Ollama;
use rusqlite::{Connection, params};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Mutex;
use zerocopy::IntoBytes;

pub struct Indexer {
    conn: Mutex<Connection>,
    ollama: Ollama,
    batch_size: usize,
}

#[derive(Debug, Clone)]
pub struct ChunkMeta {
    pub transcript_id: i64,
    pub role: String,
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
    pub sub_index: usize,
}

#[derive(Debug)]
struct TranscriptMeta {
    session_id: Option<String>,
    project_path: Option<String>,
    source: String,
    started_at: i64,
}

impl Indexer {
    pub fn new(conn: Connection, ollama: Ollama) -> Self {
        Self {
            conn: Mutex::new(conn),
            ollama,
            batch_size: 32,
        }
    }

    pub async fn index_file(&self, path: &Path) -> Result<IndexResult, IndexError> {
        let content = std::fs::read_to_string(path).map_err(IndexError::Io)?;
        let hash = hash_content(&content);

        if self.is_already_indexed(path, &hash)? {
            return Ok(IndexResult::Skipped);
        }

        let meta = parse_transcript_meta(&content, path);
        let chunks = chunk_transcript(&content);

        if chunks.is_empty() {
            return Ok(IndexResult::Empty);
        }

        let transcript_id = self.insert_transcript(path, &hash, &meta)?;
        let chunk_metas = self.insert_chunks(transcript_id, &chunks)?;
        self.embed_and_store(&chunk_metas).await?;

        Ok(IndexResult::Indexed {
            chunks: chunk_metas.len(),
        })
    }

    pub async fn index_directory(&self, dir: &Path) -> Result<DirectoryResult, IndexError> {
        let mut indexed = 0;
        let mut skipped = 0;
        let mut errors = 0;

        for entry in walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "txt"))
        {
            match self.index_file(entry.path()).await {
                Ok(IndexResult::Indexed { .. }) => indexed += 1,
                Ok(IndexResult::Skipped) => skipped += 1,
                Ok(IndexResult::Empty) => skipped += 1,
                Err(e) => {
                    tracing::warn!("failed to index {:?}: {}", entry.path(), e);
                    errors += 1;
                }
            }
        }

        Ok(DirectoryResult {
            indexed,
            skipped,
            errors,
        })
    }

    fn is_already_indexed(&self, path: &Path, hash: &str) -> Result<bool, IndexError> {
        let conn = self.conn.lock().unwrap();
        let path_str = path.to_string_lossy();

        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM transcripts WHERE file_path = ?1 AND file_hash = ?2)",
                params![path_str, hash],
                |row| row.get(0),
            )
            .map_err(IndexError::Db)?;

        Ok(exists)
    }

    fn insert_transcript(
        &self,
        path: &Path,
        hash: &str,
        meta: &TranscriptMeta,
    ) -> Result<i64, IndexError> {
        let conn = self.conn.lock().unwrap();
        let path_str = path.to_string_lossy();

        conn.execute(
            "DELETE FROM chunks WHERE transcript_id IN
             (SELECT id FROM transcripts WHERE file_path = ?1)",
            params![path_str],
        )
        .map_err(IndexError::Db)?;

        conn.execute(
            "DELETE FROM transcripts WHERE file_path = ?1",
            params![path_str],
        )
        .map_err(IndexError::Db)?;

        conn.execute(
            "INSERT INTO transcripts (session_id, project_path, source, started_at, file_path, file_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                meta.session_id,
                meta.project_path,
                meta.source,
                meta.started_at,
                path_str,
                hash
            ],
        )
        .map_err(IndexError::Db)?;

        Ok(conn.last_insert_rowid())
    }

    fn insert_chunks(
        &self,
        transcript_id: i64,
        chunks: &[Chunk],
    ) -> Result<Vec<ChunkMeta>, IndexError> {
        let conn = self.conn.lock().unwrap();
        let mut metas = Vec::with_capacity(chunks.len());

        for (idx, chunk) in chunks.iter().enumerate() {
            conn.execute(
                "INSERT INTO chunks (transcript_id, role, content, start_line, end_line, sub_index)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    transcript_id,
                    chunk.role,
                    chunk.content,
                    chunk.start_line as i64,
                    chunk.end_line as i64,
                    idx as i64
                ],
            )
            .map_err(IndexError::Db)?;

            metas.push(ChunkMeta {
                transcript_id,
                role: chunk.role.clone(),
                content: chunk.content.clone(),
                start_line: chunk.start_line,
                end_line: chunk.end_line,
                sub_index: idx,
            });
        }

        Ok(metas)
    }

    async fn embed_and_store(&self, chunks: &[ChunkMeta]) -> Result<(), IndexError> {
        for batch in chunks.chunks(self.batch_size) {
            let texts: Vec<&str> = batch.iter().map(|c| c.content.as_str()).collect();

            let embeddings = match self.ollama.embed_batch(&texts).await {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!("batch embed failed, trying one-by-one: {}", e);
                    let mut embeddings = Vec::new();
                    for text in &texts {
                        match self.ollama.embed(text).await {
                            Ok(emb) => embeddings.push(emb),
                            Err(e) => {
                                tracing::warn!("skipping chunk ({} chars): {}", text.len(), e);
                                embeddings.push(vec![0.0; 768]);
                            }
                        }
                    }
                    embeddings
                }
            };

            let conn = self.conn.lock().unwrap();

            for (chunk, embedding) in batch.iter().zip(embeddings.iter()) {
                if embedding.iter().all(|&x| x == 0.0) {
                    continue;
                }

                let chunk_id: i64 = conn
                    .query_row(
                        "SELECT id FROM chunks
                         WHERE transcript_id = ?1 AND sub_index = ?2",
                        params![chunk.transcript_id, chunk.sub_index as i64],
                        |row| row.get(0),
                    )
                    .map_err(IndexError::Db)?;

                conn.execute(
                    "INSERT INTO chunk_vectors (rowid, embedding) VALUES (?1, ?2)",
                    params![chunk_id, embedding.as_bytes()],
                )
                .map_err(IndexError::Db)?;
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum IndexResult {
    Indexed { chunks: usize },
    Skipped,
    Empty,
}

#[derive(Debug)]
pub struct DirectoryResult {
    pub indexed: usize,
    pub skipped: usize,
    pub errors: usize,
}

#[derive(Debug)]
pub enum IndexError {
    Io(std::io::Error),
    Db(rusqlite::Error),
    Embed(crate::recall::ollama::OllamaError),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexError::Io(e) => write!(f, "io error: {}", e),
            IndexError::Db(e) => write!(f, "db error: {}", e),
            IndexError::Embed(e) => write!(f, "embed error: {}", e),
        }
    }
}

impl std::error::Error for IndexError {}

#[derive(Debug)]
struct Chunk {
    role: String,
    content: String,
    start_line: usize,
    end_line: usize,
}

fn hash_content(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn parse_transcript_meta(content: &str, path: &Path) -> TranscriptMeta {
    let mut session_id = None;
    let mut project_path = None;
    let mut started_at = 0i64;

    for line in content.lines().take(10) {
        if let Some(rest) = line.strip_prefix("📋 Session: ") {
            session_id = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("📋 Project: ") {
            project_path = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("📋 Started: ") {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(rest.trim()) {
                started_at = dt.timestamp();
            }
        }
    }

    let source = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            if n.contains("-claude") {
                "claude"
            } else if n.contains("-opencode") {
                "opencode"
            } else {
                "unknown"
            }
        })
        .unwrap_or("unknown")
        .to_string();

    TranscriptMeta {
        session_id,
        project_path,
        source,
        started_at,
    }
}

fn chunk_transcript(content: &str) -> Vec<Chunk> {
    let target_size = 3000;
    let mut chunks = Vec::new();

    let mut pending_content = String::new();
    let mut pending_start = 0usize;
    let mut pending_role = String::from("mixed");

    let mut prev_boundary_content = String::new();
    let mut prev_boundary_start = 0usize;
    let mut prev_boundary_role = String::from("mixed");

    let mut last_role = String::new();

    for (line_num, line) in content.lines().enumerate() {
        let (role, _text) = parse_line(line);
        let is_turn_boundary = role.is_some();
        let is_skippable = matches!(last_role.as_str(), "meta" | "tool" | "error");

        if is_turn_boundary {
            if pending_content.chars().count() >= target_size && !prev_boundary_content.is_empty() {
                let trimmed = prev_boundary_content.trim().to_string();
                if !trimmed.is_empty() {
                    chunks.push(Chunk {
                        role: prev_boundary_role.clone(),
                        content: trimmed,
                        start_line: prev_boundary_start,
                        end_line: pending_start.saturating_sub(1),
                    });
                }
                prev_boundary_content = pending_content.clone();
                prev_boundary_start = pending_start;
                prev_boundary_role = pending_role.clone();
                pending_content.clear();
                pending_start = line_num;
            } else if !is_skippable {
                prev_boundary_content = pending_content.clone();
                prev_boundary_start = pending_start;
                prev_boundary_role = pending_role.clone();
                pending_start = line_num;
            }
        }

        if let Some(r) = &role {
            last_role = r.clone();
            if pending_content.is_empty() {
                pending_role = r.clone();
            }
        }

        if !matches!(last_role.as_str(), "meta" | "tool" | "error") {
            if !pending_content.is_empty() {
                pending_content.push('\n');
            }
            pending_content.push_str(line);
        }
    }

    if !prev_boundary_content.is_empty() {
        let trimmed = prev_boundary_content.trim().to_string();
        if !trimmed.is_empty() {
            chunks.push(Chunk {
                role: prev_boundary_role,
                content: trimmed,
                start_line: prev_boundary_start,
                end_line: pending_start.saturating_sub(1),
            });
        }
    }
    if !pending_content.is_empty() {
        let trimmed = pending_content.trim().to_string();
        if !trimmed.is_empty() {
            chunks.push(Chunk {
                role: pending_role,
                content: trimmed,
                start_line: pending_start,
                end_line: content.lines().count().saturating_sub(1),
            });
        }
    }

    chunks
}

fn parse_line(line: &str) -> (Option<String>, &str) {
    if let Some(rest) = line.strip_prefix("👤 ") {
        return (Some("user".to_string()), rest);
    }
    if let Some(rest) = line.strip_prefix("🤖 ") {
        return (Some("assistant".to_string()), rest);
    }
    if let Some(rest) = line.strip_prefix("✅ ") {
        return (Some("tool".to_string()), rest);
    }
    if let Some(rest) = line.strip_prefix("❌ ") {
        return (Some("error".to_string()), rest);
    }
    if line.starts_with("📋 ") {
        return (Some("meta".to_string()), line);
    }
    (None, line)
}
