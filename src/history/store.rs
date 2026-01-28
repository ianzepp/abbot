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
                message_type TEXT NOT NULL DEFAULT 'chat',
                timestamp INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_channel_id ON messages(channel, id DESC)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_channel_type ON messages(channel, message_type, id DESC)",
            [],
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn insert(&self, channel: &str, sender: &str, content: &str, message_type: &str) -> Result<i64, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        conn.execute(
            "INSERT INTO messages (channel, sender, content, message_type, timestamp) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![channel, sender, content, message_type, timestamp],
        )?;

        Ok(conn.last_insert_rowid())
    }

    pub fn recent(&self, channel: &str, limit: usize) -> Result<Vec<HistoryMessage>, rusqlite::Error> {
        self.recent_by_type(channel, None, limit)
    }

    pub fn recent_chat(&self, channel: &str, limit: usize) -> Result<Vec<HistoryMessage>, rusqlite::Error> {
        self.recent_by_type(channel, Some("chat"), limit)
    }

    pub fn recent_by_type(&self, channel: &str, message_type: Option<&str>, limit: usize) -> Result<Vec<HistoryMessage>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let (sql, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = match message_type {
            Some(mt) => (
                "SELECT id, channel, sender, content, timestamp
                 FROM messages
                 WHERE channel = ?1 AND message_type = ?2
                 ORDER BY id DESC
                 LIMIT ?3",
                vec![Box::new(channel.to_string()), Box::new(mt.to_string()), Box::new(limit as i64)],
            ),
            None => (
                "SELECT id, channel, sender, content, timestamp
                 FROM messages
                 WHERE channel = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
                vec![Box::new(channel.to_string()), Box::new(limit as i64)],
            ),
        };

        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
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

        store.insert("#test", "alice", "hello", "chat").unwrap();
        store.insert("#test", "bob", "hi there", "chat").unwrap();
        store.insert("#test", "tools", "file1.txt", "item").unwrap();
        store.insert("#other", "charlie", "different channel", "chat").unwrap();

        let messages = store.recent("#test", 10).unwrap();
        assert_eq!(messages.len(), 3);

        let chat_messages = store.recent_chat("#test", 10).unwrap();
        assert_eq!(chat_messages.len(), 2);
        assert_eq!(chat_messages[0].sender, "alice");
        assert_eq!(chat_messages[1].sender, "bob");

        let channels = store.channels().unwrap();
        assert!(channels.contains(&"#test".to_string()));
        assert!(channels.contains(&"#other".to_string()));
    }
}
