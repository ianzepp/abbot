use rusqlite::Connection;

pub fn ensure_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS transcripts (
            id INTEGER PRIMARY KEY,
            session_id TEXT,
            project_path TEXT,
            source TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            file_path TEXT NOT NULL UNIQUE,
            file_hash TEXT NOT NULL
        )",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_transcripts_project
         ON transcripts(project_path)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_transcripts_source
         ON transcripts(source)",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS chunks (
            id INTEGER PRIMARY KEY,
            transcript_id INTEGER NOT NULL REFERENCES transcripts(id),
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            sub_index INTEGER NOT NULL DEFAULT 0,
            UNIQUE(transcript_id, start_line, sub_index)
        )",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_chunks_transcript
         ON chunks(transcript_id)",
        [],
    )?;

    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS chunk_vectors USING vec0(
            embedding float[768]
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS recall_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_creation() {
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }

        let conn = Connection::open_in_memory().unwrap();

        ensure_schema(&conn).unwrap();

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"transcripts".to_string()));
        assert!(tables.contains(&"chunks".to_string()));
    }
}
