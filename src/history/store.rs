use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct HistoryMessage {
    pub id: i64,
    pub channel: String,
    pub sender: String,
    pub content: String,
    pub timestamp: i64,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel TEXT NOT NULL,
                sender TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_channel_id ON messages(channel, id DESC)",
            [],
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn insert(&self, channel: &str, sender: &str, content: &str) -> Result<i64, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        conn.execute(
            "INSERT INTO messages (channel, sender, content, timestamp) VALUES (?1, ?2, ?3, ?4)",
            params![channel, sender, content, timestamp],
        )?;

        Ok(conn.last_insert_rowid())
    }

    pub fn recent(&self, channel: &str, limit: usize) -> Result<Vec<HistoryMessage>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id, channel, sender, content, timestamp
             FROM messages
             WHERE channel = ?1
             ORDER BY id DESC
             LIMIT ?2"
        )?;

        let rows = stmt.query_map(params![channel, limit as i64], |row| {
            Ok(HistoryMessage {
                id: row.get(0)?,
                channel: row.get(1)?,
                sender: row.get(2)?,
                content: row.get(3)?,
                timestamp: row.get(4)?,
            })
        })?;

        let mut messages: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub fn channels(&self) -> Result<Vec<String>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare("SELECT DISTINCT channel FROM messages")?;
        let rows = stmt.query_map([], |row| row.get(0))?;

        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_roundtrip() {
        let store = Store::open(":memory:").unwrap();

        store.insert("#test", "alice", "hello").unwrap();
        store.insert("#test", "bob", "hi there").unwrap();
        store.insert("#other", "charlie", "different channel").unwrap();

        let messages = store.recent("#test", 10).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].sender, "alice");
        assert_eq!(messages[1].sender, "bob");

        let channels = store.channels().unwrap();
        assert!(channels.contains(&"#test".to_string()));
        assert!(channels.contains(&"#other".to_string()));
    }
}
