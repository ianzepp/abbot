use crate::recall::Ollama;
use rusqlite::{params, Connection};
use std::sync::Mutex;
use zerocopy::IntoBytes;

pub struct Search {
    conn: Mutex<Connection>,
    ollama: Ollama,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub chunk_id: i64,
    pub transcript_id: i64,
    pub distance: f32,
    pub role: String,
    pub content: String,
    pub project_path: Option<String>,
    pub source: String,
    pub started_at: i64,
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
}

impl Search {
    pub fn new(conn: Connection, ollama: Ollama) -> Self {
        Self {
            conn: Mutex::new(conn),
            ollama,
        }
    }

    pub async fn query(&self, text: &str, limit: usize) -> Result<Vec<SearchResult>, SearchError> {
        let embedding = self.ollama.embed(text).await.map_err(SearchError::Embed)?;
        self.query_by_vector(&embedding, limit)
    }

    pub async fn query_in_project(
        &self,
        text: &str,
        project_path: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let results = self.query(text, limit * 3).await?;

        Ok(results
            .into_iter()
            .filter(|r| {
                r.project_path
                    .as_ref()
                    .is_some_and(|p| p.contains(project_path))
            })
            .take(limit)
            .collect())
    }

    pub fn query_by_vector(
        &self,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT
                    cv.rowid as chunk_id,
                    cv.distance,
                    c.transcript_id,
                    c.role,
                    c.content,
                    c.start_line,
                    c.end_line,
                    t.project_path,
                    t.source,
                    t.started_at,
                    t.file_path
                 FROM chunk_vectors cv
                 JOIN chunks c ON c.id = cv.rowid
                 JOIN transcripts t ON t.id = c.transcript_id
                 WHERE cv.embedding MATCH ?1 AND k = ?2
                 ORDER BY cv.distance",
            )
            .map_err(SearchError::Db)?;

        let rows = stmt
            .query_map(params![embedding.as_bytes(), limit as i64], |row| {
                Ok(SearchResult {
                    chunk_id: row.get(0)?,
                    distance: row.get(1)?,
                    transcript_id: row.get(2)?,
                    role: row.get(3)?,
                    content: row.get(4)?,
                    start_line: row.get::<_, i64>(5)? as usize,
                    end_line: row.get::<_, i64>(6)? as usize,
                    project_path: row.get(7)?,
                    source: row.get(8)?,
                    started_at: row.get(9)?,
                    file_path: row.get(10)?,
                })
            })
            .map_err(SearchError::Db)?;

        rows.collect::<Result<Vec<_>, _>>().map_err(SearchError::Db)
    }

    pub fn get_context(&self, chunk_id: i64, window: usize) -> Result<Vec<String>, SearchError> {
        let conn = self.conn.lock().unwrap();

        let (transcript_id, start_line): (i64, i64) = conn
            .query_row(
                "SELECT transcript_id, start_line FROM chunks WHERE id = ?1",
                params![chunk_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(SearchError::Db)?;

        let mut stmt = conn
            .prepare(
                "SELECT content FROM chunks
                 WHERE transcript_id = ?1
                 AND start_line >= ?2 - ?3
                 AND start_line <= ?2 + ?3
                 ORDER BY start_line",
            )
            .map_err(SearchError::Db)?;

        let rows = stmt
            .query_map(
                params![transcript_id, start_line, (window as i64) * 100],
                |row| row.get(0),
            )
            .map_err(SearchError::Db)?;

        rows.collect::<Result<Vec<_>, _>>().map_err(SearchError::Db)
    }

    pub fn stats(&self) -> Result<MemoryStats, SearchError> {
        let conn = self.conn.lock().unwrap();

        let transcript_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM transcripts", [], |row| row.get(0))
            .map_err(SearchError::Db)?;

        let chunk_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
            .map_err(SearchError::Db)?;

        let vector_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunk_vectors", [], |row| row.get(0))
            .map_err(SearchError::Db)?;

        Ok(MemoryStats {
            transcripts: transcript_count as usize,
            chunks: chunk_count as usize,
            vectors: vector_count as usize,
        })
    }
}

#[derive(Debug)]
pub struct MemoryStats {
    pub transcripts: usize,
    pub chunks: usize,
    pub vectors: usize,
}

#[derive(Debug)]
pub enum SearchError {
    Db(rusqlite::Error),
    Embed(crate::recall::ollama::OllamaError),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchError::Db(e) => write!(f, "db error: {}", e),
            SearchError::Embed(e) => write!(f, "embed error: {}", e),
        }
    }
}

impl std::error::Error for SearchError {}
