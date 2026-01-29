use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;
use crate::bus::{Message, MessageOp, MessageData};

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                op TEXT NOT NULL,
                sender TEXT NOT NULL,
                channel TEXT NOT NULL,
                data TEXT NOT NULL,
                reply_to TEXT,
                timestamp INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_channel_ts ON messages(channel, timestamp DESC)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_channel_op ON messages(channel, op, timestamp DESC)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_reply_to ON messages(reply_to)",
            [],
        )?;

        // Monk registry - persistent monk existence
        conn.execute(
            "CREATE TABLE IF NOT EXISTS monks (
                id TEXT PRIMARY KEY,
                model TEXT NOT NULL DEFAULT 'sonnet',
                created_at INTEGER NOT NULL
            )",
            [],
        )?;

        // Layer 2: monk's self-managed identity/memory
        conn.execute(
            "CREATE TABLE IF NOT EXISTS monk_self (
                monk_id TEXT PRIMARY KEY,
                content TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;

        // Layer 3: monk's per-channel workspace
        conn.execute(
            "CREATE TABLE IF NOT EXISTS monk_workspace (
                monk_id TEXT NOT NULL,
                channel TEXT NOT NULL,
                content TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (monk_id, channel)
            )",
            [],
        )?;

        // Garden: monk's personal collection of thoughts/observations
        conn.execute(
            "CREATE TABLE IF NOT EXISTS monk_garden (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                monk_id TEXT NOT NULL,
                content TEXT NOT NULL,
                planted_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_garden_monk ON monk_garden(monk_id, planted_at DESC)",
            [],
        )?;

        // Key-value metadata storage
        conn.execute(
            "CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
            [],
        )?;

        // Tool call logging for debugging and analytics
        conn.execute(
            "CREATE TABLE IF NOT EXISTS tool_calls (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                monk_id TEXT NOT NULL,
                batch_id TEXT NOT NULL,
                iteration INTEGER NOT NULL,
                position INTEGER NOT NULL,
                tool TEXT NOT NULL,
                reason TEXT,
                content TEXT NOT NULL,
                output TEXT NOT NULL,
                success INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                timestamp INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_tool_calls_monk ON tool_calls(monk_id, timestamp DESC)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_tool_calls_batch ON tool_calls(batch_id, iteration, position)",
            [],
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;

        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn insert(&self, msg: &Message) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let op = format!("{:?}", msg.op);
        let data = serde_json::to_string(&msg.data).unwrap_or_default();
        let reply_to = msg.reply_to.map(|u| u.to_string());
        let timestamp = msg.timestamp
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        conn.execute(
            "INSERT INTO messages (id, op, sender, channel, data, reply_to, timestamp) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![msg.id.to_string(), op, msg.sender, msg.channel, data, reply_to, timestamp],
        )?;

        Ok(())
    }

    pub fn recent(&self, channel: &str, limit: usize) -> Result<Vec<Message>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id, op, sender, channel, data, reply_to, timestamp
             FROM messages
             WHERE channel = ?1
             ORDER BY timestamp DESC
             LIMIT ?2"
        )?;

        let rows = stmt.query_map(params![channel, limit as i64], |row| {
            Self::row_to_message(row)
        })?;

        let mut messages: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub fn recent_by_op(&self, channel: &str, op: &str, limit: usize) -> Result<Vec<Message>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id, op, sender, channel, data, reply_to, timestamp
             FROM messages
             WHERE channel = ?1 AND op = ?2
             ORDER BY timestamp DESC
             LIMIT ?3"
        )?;

        let rows = stmt.query_map(params![channel, op, limit as i64], |row| {
            Self::row_to_message(row)
        })?;

        let mut messages: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub fn recent_chat(&self, channel: &str, limit: usize) -> Result<Vec<Message>, rusqlite::Error> {
        self.recent_by_op(channel, "Chat", limit)
    }

    pub fn search(&self, channel: &str, query: &str, limit: usize) -> Result<Vec<Message>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id, op, sender, channel, data, reply_to, timestamp
             FROM messages
             WHERE channel = ?1 AND data LIKE ?2
             ORDER BY timestamp DESC
             LIMIT ?3"
        )?;

        let pattern = format!("%{}%", query);
        let rows = stmt.query_map(params![channel, pattern, limit as i64], |row| {
            Self::row_to_message(row)
        })?;

        let mut messages: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub fn recent_from(&self, channel: &str, sender: &str, limit: usize) -> Result<Vec<Message>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id, op, sender, channel, data, reply_to, timestamp
             FROM messages
             WHERE channel = ?1 AND sender = ?2
             ORDER BY timestamp DESC
             LIMIT ?3"
        )?;

        let rows = stmt.query_map(params![channel, sender, limit as i64], |row| {
            Self::row_to_message(row)
        })?;

        let mut messages: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub fn get_thread(&self, reply_to: Uuid) -> Result<Vec<Message>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id, op, sender, channel, data, reply_to, timestamp
             FROM messages
             WHERE reply_to = ?1
             ORDER BY timestamp ASC"
        )?;

        let rows = stmt.query_map(params![reply_to.to_string()], |row| {
            Self::row_to_message(row)
        })?;

        rows.collect()
    }

    pub fn get(&self, id: Uuid) -> Result<Option<Message>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id, op, sender, channel, data, reply_to, timestamp
             FROM messages
             WHERE id = ?1"
        )?;

        let mut rows = stmt.query_map(params![id.to_string()], |row| {
            Self::row_to_message(row)
        })?;

        match rows.next() {
            Some(Ok(msg)) => Ok(Some(msg)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    fn row_to_message(row: &rusqlite::Row) -> Result<Message, rusqlite::Error> {
        let id_str: String = row.get(0)?;
        let op_str: String = row.get(1)?;
        let sender: String = row.get(2)?;
        let channel: String = row.get(3)?;
        let data_str: String = row.get(4)?;
        let reply_to_str: Option<String> = row.get(5)?;
        let timestamp_ms: i64 = row.get(6)?;

        let id = Uuid::parse_str(&id_str).unwrap_or_else(|_| Uuid::new_v4());
        let op = Self::parse_op(&op_str);
        let data: MessageData = serde_json::from_str(&data_str).unwrap_or(MessageData::Empty);
        let reply_to = reply_to_str.and_then(|s| Uuid::parse_str(&s).ok());
        let timestamp = std::time::UNIX_EPOCH + std::time::Duration::from_millis(timestamp_ms as u64);

        Ok(Message {
            id,
            op,
            sender,
            channel,
            data,
            reply_to,
            timestamp,
        })
    }

    fn parse_op(s: &str) -> MessageOp {
        match s {
            "Ok" => MessageOp::Ok,
            "Error" => MessageOp::Error,
            "Done" => MessageOp::Done,
            "Item" => MessageOp::Item,
            "Data" => MessageOp::Data,
            "Event" => MessageOp::Event,
            "Progress" => MessageOp::Progress,
            "Chat" => MessageOp::Chat,
            "Exec" => MessageOp::Exec,
            "Ping" => MessageOp::Ping,
            _ => MessageOp::Chat,
        }
    }

    // Monk registry

    pub fn create_monk(&self, id: &str, model: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        conn.execute(
            "INSERT OR IGNORE INTO monks (id, model, created_at) VALUES (?1, ?2, ?3)",
            params![id, model, now],
        )?;
        Ok(())
    }

    pub fn list_monks(&self) -> Result<Vec<(String, String)>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, model FROM monks ORDER BY created_at")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect()
    }

    pub fn delete_monk(&self, id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM monks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn monk_exists(&self, id: &str) -> Result<bool, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT 1 FROM monks WHERE id = ?1")?;
        let exists = stmt.exists(params![id])?;
        Ok(exists)
    }

    // Layer 2: monk self

    pub fn get_monk_self(&self, monk_id: &str) -> Result<String, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT content FROM monk_self WHERE monk_id = ?1")?;
        let result: Result<String, _> = stmt.query_row(params![monk_id], |row| row.get(0));
        Ok(result.unwrap_or_default())
    }

    pub fn set_monk_self(&self, monk_id: &str, content: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        conn.execute(
            "INSERT INTO monk_self (monk_id, content, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(monk_id) DO UPDATE SET content = ?2, updated_at = ?3",
            params![monk_id, content, now],
        )?;
        Ok(())
    }

    pub fn delete_monk_self(&self, monk_id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM monk_self WHERE monk_id = ?1", params![monk_id])?;
        Ok(())
    }

    // Layer 3: monk workspace

    pub fn get_workspace(&self, monk_id: &str, channel: &str) -> Result<String, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT content FROM monk_workspace WHERE monk_id = ?1 AND channel = ?2"
        )?;
        let result: Result<String, _> = stmt.query_row(params![monk_id, channel], |row| row.get(0));
        Ok(result.unwrap_or_default())
    }

    pub fn set_workspace(&self, monk_id: &str, channel: &str, content: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        conn.execute(
            "INSERT INTO monk_workspace (monk_id, channel, content, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(monk_id, channel) DO UPDATE SET content = ?3, updated_at = ?4",
            params![monk_id, channel, content, now],
        )?;
        Ok(())
    }

    pub fn delete_workspace(&self, monk_id: &str, channel: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM monk_workspace WHERE monk_id = ?1 AND channel = ?2",
            params![monk_id, channel],
        )?;
        Ok(())
    }

    pub fn delete_all_workspaces(&self, monk_id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM monk_workspace WHERE monk_id = ?1", params![monk_id])?;
        Ok(())
    }

    pub fn list_monk_channels(&self, monk_id: &str) -> Result<Vec<String>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT channel FROM monk_workspace WHERE monk_id = ?1 ORDER BY updated_at DESC"
        )?;
        let rows = stmt.query_map(params![monk_id], |row| row.get(0))?;
        rows.collect()
    }

    // Garden

    pub fn garden_plant(&self, monk_id: &str, content: &str) -> Result<i64, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        conn.execute(
            "INSERT INTO monk_garden (monk_id, content, planted_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![monk_id, content, now],
        )?;

        Ok(conn.last_insert_rowid())
    }

    /// Returns (id, content, age_in_days)
    pub fn garden_list(&self, monk_id: &str) -> Result<Vec<(i64, String, i64)>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let mut stmt = conn.prepare(
            "SELECT id, content, planted_at FROM monk_garden WHERE monk_id = ?1 ORDER BY planted_at DESC"
        )?;

        let rows = stmt.query_map(params![monk_id], |row| {
            let id: i64 = row.get(0)?;
            let content: String = row.get(1)?;
            let planted_at: i64 = row.get(2)?;
            let age_days = (now - planted_at) / (1000 * 60 * 60 * 24);
            Ok((id, content, age_days))
        })?;

        rows.collect()
    }

    pub fn garden_prune(&self, monk_id: &str, id: i64) -> Result<bool, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "DELETE FROM monk_garden WHERE id = ?1 AND monk_id = ?2",
            params![id, monk_id],
        )?;
        Ok(affected > 0)
    }

    pub fn garden_water(&self, monk_id: &str, id: i64, content: &str) -> Result<bool, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let affected = conn.execute(
            "UPDATE monk_garden SET content = ?1, updated_at = ?2 WHERE id = ?3 AND monk_id = ?4",
            params![content, now, id, monk_id],
        )?;
        Ok(affected > 0)
    }

    pub fn garden_clear(&self, monk_id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM monk_garden WHERE monk_id = ?1", params![monk_id])?;
        Ok(())
    }

    // Tool call logging

    pub fn log_tool_call(
        &self,
        monk_id: &str,
        batch_id: &str,
        iteration: usize,
        position: usize,
        tool: &str,
        reason: Option<&str>,
        content: &str,
        output: &str,
        success: bool,
        duration_ms: u64,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        conn.execute(
            "INSERT INTO tool_calls (monk_id, batch_id, iteration, position, tool, reason, content, output, success, duration_ms, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![monk_id, batch_id, iteration as i64, position as i64, tool, reason, content, output, success as i32, duration_ms as i64, now],
        )?;

        Ok(())
    }

    /// Get recent tool calls for a monk, ordered by batch/iteration/position
    pub fn recent_tool_calls(&self, monk_id: &str, limit: usize) -> Result<Vec<ToolCallRecord>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, batch_id, iteration, position, tool, reason, content, output, success, duration_ms, timestamp
             FROM tool_calls WHERE monk_id = ?1 ORDER BY timestamp DESC, iteration, position LIMIT ?2"
        )?;

        let rows = stmt.query_map(params![monk_id, limit as i64], |row| {
            Ok(ToolCallRecord {
                id: row.get(0)?,
                batch_id: row.get(1)?,
                iteration: row.get::<_, i64>(2)? as usize,
                position: row.get::<_, i64>(3)? as usize,
                tool: row.get(4)?,
                reason: row.get(5)?,
                content: row.get(6)?,
                output: row.get(7)?,
                success: row.get::<_, i32>(8)? != 0,
                duration_ms: row.get::<_, i64>(9)? as u64,
                timestamp: row.get(10)?,
            })
        })?;

        rows.collect()
    }

    /// Get tool calls for a specific batch (one on_message invocation)
    pub fn batch_tool_calls(&self, batch_id: &str) -> Result<Vec<ToolCallRecord>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, batch_id, iteration, position, tool, reason, content, output, success, duration_ms, timestamp
             FROM tool_calls WHERE batch_id = ?1 ORDER BY iteration, position"
        )?;

        let rows = stmt.query_map(params![batch_id], |row| {
            Ok(ToolCallRecord {
                id: row.get(0)?,
                batch_id: row.get(1)?,
                iteration: row.get::<_, i64>(2)? as usize,
                position: row.get::<_, i64>(3)? as usize,
                tool: row.get(4)?,
                reason: row.get(5)?,
                content: row.get(6)?,
                output: row.get(7)?,
                success: row.get::<_, i32>(8)? != 0,
                duration_ms: row.get::<_, i64>(9)? as u64,
                timestamp: row.get(10)?,
            })
        })?;

        rows.collect()
    }

    /// Count tool calls by reason (for detecting repetition)
    pub fn tool_call_stats(&self, monk_id: &str, since_ms: i64) -> Result<Vec<(String, String, i64)>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT tool, reason, COUNT(*) as count
             FROM tool_calls
             WHERE monk_id = ?1 AND timestamp > ?2
             GROUP BY tool, reason
             ORDER BY count DESC
             LIMIT 50"
        )?;

        let rows = stmt.query_map(params![monk_id, since_ms], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get::<_, i64>(2)?,
            ))
        })?;

        rows.collect()
    }

    /// List all channels with message counts
    pub fn list_channels(&self) -> Result<Vec<(String, i64)>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT channel, COUNT(*) as cnt FROM messages GROUP BY channel ORDER BY cnt DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;

        rows.collect()
    }
}

#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub id: i64,
    pub batch_id: String,
    pub iteration: usize,
    pub position: usize,
    pub tool: String,
    pub reason: Option<String>,
    pub content: String,
    pub output: String,
    pub success: bool,
    pub duration_ms: u64,
    pub timestamp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::respond;

    #[test]
    fn test_store_roundtrip() {
        let store = Store::open(":memory:").unwrap();

        let msg1 = respond::chat("alice", "#test", "hello");
        let msg2 = respond::chat("bob", "#test", "hi there");
        let msg3 = respond::exec("alice", "#test", "bash", "ls -la");

        store.insert(&msg1).unwrap();
        store.insert(&msg2).unwrap();
        store.insert(&msg3).unwrap();

        let messages = store.recent("#test", 10).unwrap();
        assert_eq!(messages.len(), 3);

        let alice_msg = messages.iter().find(|m| m.sender == "alice" && m.op == MessageOp::Chat).unwrap();
        assert_eq!(alice_msg.text(), Some("hello"));

        let chat_messages = store.recent_chat("#test", 10).unwrap();
        assert_eq!(chat_messages.len(), 2);

        let exec_messages = store.recent_by_op("#test", "Exec", 10).unwrap();
        assert_eq!(exec_messages.len(), 1);
        if let MessageData::Exec { tool, args } = &exec_messages[0].data {
            assert_eq!(tool, "bash");
            assert_eq!(args, "ls -la");
        } else {
            panic!("expected Exec data");
        }
    }

    #[test]
    fn test_thread_lookup() {
        let store = Store::open(":memory:").unwrap();

        let original = respond::exec("alice", "#test", "bash", "ls");
        let reply1 = respond::item_text("tools", "#test", "file1.txt").with_reply_to(original.id);
        let reply2 = respond::item_text("tools", "#test", "file2.txt").with_reply_to(original.id);
        let reply3 = respond::ok_text("tools", "#test", "").with_reply_to(original.id);

        store.insert(&original).unwrap();
        store.insert(&reply1).unwrap();
        store.insert(&reply2).unwrap();
        store.insert(&reply3).unwrap();

        let thread = store.get_thread(original.id).unwrap();
        assert_eq!(thread.len(), 3);
        assert_eq!(thread[0].op, MessageOp::Item);
        assert_eq!(thread[2].op, MessageOp::Ok);
    }

    #[test]
    fn test_get_by_id() {
        let store = Store::open(":memory:").unwrap();

        let msg = respond::chat("alice", "#test", "hello");
        let id = msg.id;
        store.insert(&msg).unwrap();

        let retrieved = store.get(id).unwrap().unwrap();
        assert_eq!(retrieved.id, id);
        assert_eq!(retrieved.text(), Some("hello"));
    }
}
