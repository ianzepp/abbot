use crate::bus::{Message, MessageData, MessageOp, Scope};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let mut conn = Connection::open(path)?;

        ensure_messages_schema(&mut conn)?;

        // Head memory (global per head)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS head_memory (
                head_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                content TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (head_id, kind)
            )",
            [],
        )?;

        // Hand execution log (tool calls made by hands during tasks)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS hand_exec (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                hand_id TEXT NOT NULL,
                step INTEGER NOT NULL,
                tool TEXT NOT NULL,
                args TEXT NOT NULL,
                output TEXT NOT NULL,
                success INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                hand_thought TEXT NOT NULL DEFAULT '',
                timestamp INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_hand_exec_task ON hand_exec(task_id, step ASC)",
            [],
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn get_head_memory(&self, head_id: &str, kind: &str) -> Result<String, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT content FROM head_memory WHERE head_id = ?1 AND kind = ?2")?;
        let result: Result<String, _> = stmt.query_row(params![head_id, kind], |row| row.get(0));
        Ok(result.unwrap_or_default())
    }

    pub fn set_head_memory(
        &self,
        head_id: &str,
        kind: &str,
        content: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        conn.execute(
            "INSERT INTO head_memory (head_id, kind, content, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(head_id, kind) DO UPDATE SET content = ?3, updated_at = ?4",
            params![head_id, kind, content, now],
        )?;
        Ok(())
    }

    pub fn get_head_ltm(&self, head_id: &str) -> Result<String, rusqlite::Error> {
        self.get_head_memory(head_id, "ltm")
    }

    pub fn set_head_ltm(&self, head_id: &str, content: &str) -> Result<(), rusqlite::Error> {
        self.set_head_memory(head_id, "ltm", content)
    }

    pub fn get_head_stm(&self, head_id: &str) -> Result<String, rusqlite::Error> {
        self.get_head_memory(head_id, "stm")
    }

    pub fn set_head_stm(&self, head_id: &str, content: &str) -> Result<(), rusqlite::Error> {
        self.set_head_memory(head_id, "stm", content)
    }

    pub fn insert(&self, msg: &Message) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let op = format!("{:?}", msg.op);
        let origin = msg.origin.as_str();
        let data = serde_json::to_string(&msg.data).unwrap_or_default();
        let reply_to = msg.reply_to.map(|u| u.to_string());
        let timestamp = msg
            .timestamp
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        conn.execute(
            "INSERT INTO messages (id, op, origin, sender, scope_type, scope_key, data, reply_to, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                msg.id.to_string(),
                op,
                origin,
                msg.sender,
                msg.scope.kind_str(),
                msg.scope.key(),
                data,
                reply_to,
                timestamp
            ],
        )?;

        Ok(())
    }

    pub fn recent(&self, scope: &str, limit: usize) -> Result<Vec<Message>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let scope = Scope::from(scope);

        let mut stmt = conn.prepare(
            "SELECT id, op, origin, sender, scope_type, scope_key, data, reply_to, timestamp
             FROM messages
             WHERE scope_type = ?1 AND scope_key = ?2
             ORDER BY timestamp DESC
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(
            params![scope.kind_str(), scope.key(), limit as i64],
            |row| Self::row_to_message(row),
        )?;

        let mut messages: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub fn recent_by_op(
        &self,
        scope: &str,
        op: &str,
        limit: usize,
    ) -> Result<Vec<Message>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let scope = Scope::from(scope);

        let mut stmt = conn.prepare(
            "SELECT id, op, origin, sender, scope_type, scope_key, data, reply_to, timestamp
             FROM messages
             WHERE scope_type = ?1 AND scope_key = ?2 AND op = ?3
             ORDER BY timestamp DESC
             LIMIT ?4",
        )?;

        let rows = stmt.query_map(
            params![scope.kind_str(), scope.key(), op, limit as i64],
            |row| Self::row_to_message(row),
        )?;

        let mut messages: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub fn recent_chat(&self, scope: &str, limit: usize) -> Result<Vec<Message>, rusqlite::Error> {
        self.recent_by_op(scope, "Chat", limit)
    }

    pub fn search(
        &self,
        scope: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Message>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let scope = Scope::from(scope);

        let mut stmt = conn.prepare(
            "SELECT id, op, origin, sender, scope_type, scope_key, data, reply_to, timestamp
             FROM messages
             WHERE scope_type = ?1 AND scope_key = ?2 AND data LIKE ?3
             ORDER BY timestamp DESC
             LIMIT ?4",
        )?;

        let pattern = format!("%{}%", query);
        let rows = stmt.query_map(
            params![scope.kind_str(), scope.key(), pattern, limit as i64],
            |row| Self::row_to_message(row),
        )?;

        let mut messages: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub fn recent_from(
        &self,
        scope: &str,
        sender: &str,
        limit: usize,
    ) -> Result<Vec<Message>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let scope = Scope::from(scope);

        let mut stmt = conn.prepare(
            "SELECT id, op, origin, sender, scope_type, scope_key, data, reply_to, timestamp
             FROM messages
             WHERE scope_type = ?1 AND scope_key = ?2 AND sender = ?3
             ORDER BY timestamp DESC
             LIMIT ?4",
        )?;

        let rows = stmt.query_map(
            params![scope.kind_str(), scope.key(), sender, limit as i64],
            |row| Self::row_to_message(row),
        )?;

        let mut messages: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub fn get_thread(&self, reply_to: Uuid) -> Result<Vec<Message>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id, op, origin, sender, scope_type, scope_key, data, reply_to, timestamp
             FROM messages
             WHERE reply_to = ?1
             ORDER BY timestamp ASC",
        )?;

        let rows = stmt.query_map(params![reply_to.to_string()], |row| {
            Self::row_to_message(row)
        })?;

        rows.collect()
    }

    pub fn get(&self, id: Uuid) -> Result<Option<Message>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id, op, origin, sender, scope_type, scope_key, data, reply_to, timestamp
             FROM messages
             WHERE id = ?1",
        )?;

        let mut rows = stmt.query_map(params![id.to_string()], |row| Self::row_to_message(row))?;

        match rows.next() {
            Some(Ok(msg)) => Ok(Some(msg)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    fn row_to_message(row: &rusqlite::Row) -> Result<Message, rusqlite::Error> {
        let id_str: String = row.get(0)?;
        let op_str: String = row.get(1)?;
        let origin_str: String = row.get(2)?;
        let sender: String = row.get(3)?;
        let scope_type: String = row.get(4)?;
        let scope_key: String = row.get(5)?;
        let data_str: String = row.get(6)?;
        let reply_to_str: Option<String> = row.get(7)?;
        let timestamp_ms: i64 = row.get(8)?;

        let id = Uuid::parse_str(&id_str).unwrap_or_else(|_| Uuid::new_v4());
        let op = Self::parse_op(&op_str);
        let origin = crate::bus::Origin::from_str(&origin_str);
        let data: MessageData = serde_json::from_str(&data_str).unwrap_or(MessageData::Empty);
        let reply_to = reply_to_str.and_then(|s| Uuid::parse_str(&s).ok());
        let timestamp =
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(timestamp_ms as u64);
        let scope = scope_from_parts(&scope_type, &scope_key);

        Ok(Message {
            id,
            op,
            origin,
            sender,
            scope,
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
            "Task" => MessageOp::Task,
            _ => MessageOp::Chat,
        }
    }

    pub fn log_hand_exec(
        &self,
        task_id: &str,
        hand_id: &str,
        step: usize,
        tool: &str,
        args: &str,
        output: &str,
        success: bool,
        duration_ms: u64,
        hand_thought: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        conn.execute(
            "INSERT INTO hand_exec (task_id, hand_id, step, tool, args, output, success, duration_ms, hand_thought, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task_id,
                hand_id,
                step as i64,
                tool,
                args,
                output,
                success as i32,
                duration_ms as i64,
                hand_thought,
                now
            ],
        )?;

        Ok(())
    }

    /// Get all exec records for a task, ordered by step
    pub fn get_hand_execs(&self, task_id: &str) -> Result<Vec<HandExec>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, task_id, hand_id, step, tool, args, output, success, hand_thought
             FROM hand_exec WHERE task_id = ?1 ORDER BY step ASC",
        )?;

        let rows = stmt.query_map(params![task_id], |row| {
            Ok(HandExec {
                id: row.get(0)?,
                task_id: row.get(1)?,
                hand_id: row.get(2)?,
                step: row.get::<_, i64>(3)? as usize,
                tool: row.get(4)?,
                args: row.get(5)?,
                output: row.get(6)?,
                success: row.get::<_, i32>(7)? != 0,
                hand_thought: row.get(8)?,
            })
        })?;

        rows.collect()
    }

    /// List all channels with message counts
    pub fn list_channels(&self) -> Result<Vec<(String, i64)>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT scope_key, COUNT(*) as cnt
             FROM messages
             WHERE scope_type = 'channel'
             GROUP BY scope_key
             ORDER BY cnt DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            let key: String = row.get(0)?;
            let cnt: i64 = row.get(1)?;
            Ok((format!("#{}", key), cnt))
        })?;

        rows.collect()
    }
}

fn scope_from_parts(scope_type: &str, scope_key: &str) -> Scope {
    match scope_type {
        "channel" => Scope::Channel(scope_key.to_string()),
        "mail" => Scope::Mail(scope_key.to_string()),
        "task" => Scope::Task(scope_key.to_string()),
        _ => Scope::Channel(scope_key.to_string()),
    }
}

fn ensure_messages_schema(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='messages')",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0;

    if !exists {
        create_messages_v2_conn(conn)?;
        return Ok(());
    }

    let cols = {
        let mut stmt = conn.prepare("PRAGMA table_info(messages)")?;
        stmt.query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
    };

    if cols.iter().any(|c| c == "scope_type") && cols.iter().any(|c| c == "scope_key") {
        ensure_messages_origin_column(conn, &cols)?;
        return Ok(());
    }

    if cols.iter().any(|c| c == "channel") {
        migrate_messages_v1_to_v2(conn)?;
        return Ok(());
    }

    migrate_messages_v1_to_v2(conn)?;
    Ok(())
}

fn ensure_messages_origin_column(
    conn: &mut Connection,
    cols: &[String],
) -> Result<(), rusqlite::Error> {
    if cols.iter().any(|c| c == "origin") {
        return Ok(());
    }
    conn.execute(
        "ALTER TABLE messages ADD COLUMN origin TEXT NOT NULL DEFAULT 'system'",
        [],
    )?;
    Ok(())
}

fn create_messages_v2_sql(
    mut execute: impl FnMut(&str) -> Result<(), rusqlite::Error>,
) -> Result<(), rusqlite::Error> {
    execute(
        "CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            op TEXT NOT NULL,
            origin TEXT NOT NULL,
            sender TEXT NOT NULL,
            scope_type TEXT NOT NULL,
            scope_key TEXT NOT NULL,
            data TEXT NOT NULL,
            reply_to TEXT,
            timestamp INTEGER NOT NULL
        )",
    )?;
    execute("CREATE INDEX IF NOT EXISTS idx_scope_ts ON messages(scope_type, scope_key, timestamp DESC)")?;
    execute("CREATE INDEX IF NOT EXISTS idx_scope_op ON messages(scope_type, scope_key, op, timestamp DESC)")?;
    execute("CREATE INDEX IF NOT EXISTS idx_reply_to ON messages(reply_to)")?;
    Ok(())
}

fn create_messages_v2_conn(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            op TEXT NOT NULL,
            origin TEXT NOT NULL,
            sender TEXT NOT NULL,
            scope_type TEXT NOT NULL,
            scope_key TEXT NOT NULL,
            data TEXT NOT NULL,
            reply_to TEXT,
            timestamp INTEGER NOT NULL
        )",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_scope_ts ON messages(scope_type, scope_key, timestamp DESC)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_scope_op ON messages(scope_type, scope_key, op, timestamp DESC)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_reply_to ON messages(reply_to)",
        [],
    )?;

    Ok(())
}

fn migrate_messages_v1_to_v2(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction()?;

    tx.execute("ALTER TABLE messages RENAME TO messages_old", [])?;

    create_messages_v2_sql(|sql| tx.execute(sql, []).map(|_| ()))?;

    let rows: Vec<(String, String, String, String, String, Option<String>, i64)> = {
        let mut stmt = tx.prepare(
            "SELECT id, op, sender, channel, data, reply_to, timestamp FROM messages_old",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    for (id, op, sender, channel, data, reply_to, timestamp) in rows {
        let scope = Scope::from(channel.as_str());
        tx.execute(
            "INSERT INTO messages (id, op, origin, sender, scope_type, scope_key, data, reply_to, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                op,
                "system",
                sender,
                scope.kind_str(),
                scope.key(),
                data,
                reply_to,
                timestamp
            ],
        )?;
    }

    tx.execute("DROP TABLE messages_old", [])?;
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct HandExec {
    pub id: i64,
    pub task_id: String,
    pub hand_id: String,
    pub step: usize,
    pub tool: String,
    pub args: String,
    pub output: String,
    pub success: bool,
    pub hand_thought: String,
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

        let alice_msg = messages
            .iter()
            .find(|m| m.sender == "alice" && m.op == MessageOp::Chat)
            .unwrap();
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
