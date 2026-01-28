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

        Ok(Self {
            conn: Mutex::new(conn),
        })
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
