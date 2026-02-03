use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Want {
    pub id: String,
    pub want: String,
    pub context: String,
    pub priority: String,
    pub source: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct ConclaveRecord {
    pub id: String,
    pub status: String,
    pub transcript: String,
    pub decision: String,
    pub created_at: i64,
}

pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct ToolRegistryTool {
    pub name: String,
    pub summary: String,
    pub description: String,
    pub schema_json: String,
}

#[derive(Debug, Clone)]
pub struct ToolRegistrySummary {
    pub name: String,
    pub summary: String,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;

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

        // Raw LLM interactions (provider request/response JSON) for replay/debugging.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS llm_interaction (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent TEXT NOT NULL,
                run_id TEXT NOT NULL,
                iter INTEGER NOT NULL,
                request_json TEXT NOT NULL,
                response_json TEXT NOT NULL,
                timestamp INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_llm_interaction_run ON llm_interaction(agent, run_id, iter ASC)",
            [],
        )?;

        // Wants pool (aspirational items Mind can promote to needs)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS wants (
                id TEXT PRIMARY KEY,
                want TEXT NOT NULL,
                context TEXT NOT NULL DEFAULT '',
                priority TEXT NOT NULL DEFAULT 'normal',
                source TEXT NOT NULL DEFAULT 'mind',
                created_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_wants_priority ON wants(priority, created_at ASC)",
            [],
        )?;

        // Conclave self identity (collective identity definition)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS conclave_self (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                content TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;

        // Conclave sessions (deliberation history)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS conclaves (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                transcript TEXT NOT NULL,
                decision TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_conclaves_created ON conclaves(created_at DESC)",
            [],
        )?;

        // Tool registry (per scope)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS tool_registry (
                scope TEXT NOT NULL,
                source TEXT NOT NULL,
                name TEXT NOT NULL,
                summary TEXT NOT NULL,
                description TEXT NOT NULL,
                schema_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (scope, source, name)
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_tool_registry_scope ON tool_registry(scope, source)",
            [],
        )?;

        if cfg!(debug_assertions) {
            conn.execute("DELETE FROM tool_registry", [])?;
        }

        // Per-session state for OpenAI-compatible clients.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS session_state (
                scope TEXT PRIMARY KEY,
                active_thread_id TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS session_env (
                scope TEXT PRIMARY KEY,
                env_block TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS session_model (
                scope TEXT PRIMARY KEY,
                model TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;

        if cfg!(debug_assertions) {
            conn.execute("DELETE FROM session_state", [])?;
            conn.execute("DELETE FROM session_env", [])?;
            conn.execute("DELETE FROM session_model", [])?;
        }

        // User system prompt cache + scope mapping.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_prompt_cache (
                hash TEXT PRIMARY KEY,
                prompt TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS session_prompt (
                scope TEXT PRIMARY KEY,
                prompt_hash TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY(prompt_hash) REFERENCES user_prompt_cache(hash)
            )",
            [],
        )?;

        if cfg!(debug_assertions) {
            // Delete the child table first to avoid foreign key violations when clearing fixtures.
            conn.execute("DELETE FROM session_prompt", [])?;
            conn.execute("DELETE FROM user_prompt_cache", [])?;
        }

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn set_session_env(&self, scope: &str, env_block: &str) -> Result<(), rusqlite::Error> {
        let scope = scope.trim();
        if scope.is_empty() {
            return Ok(());
        }
        let env_block = env_block.trim();
        if env_block.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        conn.execute(
            "INSERT INTO session_env (scope, env_block, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(scope) DO UPDATE SET env_block = ?2, updated_at = ?3",
            params![scope, env_block, now],
        )?;
        Ok(())
    }

    pub fn get_session_env(&self, scope: &str) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT env_block FROM session_env WHERE scope = ?1")?;
        let result: Result<String, _> = stmt.query_row(params![scope], |row| row.get(0));
        match result {
            Ok(s) => {
                let s = s.trim().to_string();
                if s.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(s))
                }
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn set_session_model(&self, scope: &str, model: &str) -> Result<(), rusqlite::Error> {
        let scope = scope.trim();
        if scope.is_empty() {
            return Ok(());
        }
        let model = model.trim();
        if model.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        conn.execute(
            "INSERT INTO session_model (scope, model, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(scope) DO UPDATE SET model = ?2, updated_at = ?3",
            params![scope, model, now],
        )?;
        Ok(())
    }

    pub fn get_session_model(&self, scope: &str) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT model FROM session_model WHERE scope = ?1")?;
        let result: Result<String, _> = stmt.query_row(params![scope], |row| row.get(0));
        match result {
            Ok(s) => {
                let s = s.trim().to_string();
                if s.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(s))
                }
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn clear_session_model(&self, scope: &str) -> Result<(), rusqlite::Error> {
        let scope = scope.trim();
        if scope.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM session_model WHERE scope = ?1", params![scope])?;
        Ok(())
    }

    pub fn set_active_thread(&self, scope: &str, thread_id: Uuid) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        conn.execute(
            "INSERT INTO session_state (scope, active_thread_id, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(scope) DO UPDATE SET active_thread_id = ?2, updated_at = ?3",
            params![scope, thread_id.to_string(), now],
        )?;
        Ok(())
    }

    pub fn get_active_thread(&self, scope: &str) -> Result<Option<Uuid>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT active_thread_id FROM session_state WHERE scope = ?1")?;
        let result: Result<String, _> = stmt.query_row(params![scope], |row| row.get(0));
        match result {
            Ok(s) => Ok(Uuid::parse_str(&s).ok()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn replace_external_tools(
        &self,
        scope: &str,
        tools: &[ToolRegistryTool],
    ) -> Result<(), rusqlite::Error> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        tx.execute(
            "DELETE FROM tool_registry WHERE scope = ?1 AND source = 'external'",
            params![scope],
        )?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        for t in tools {
            tx.execute(
                "INSERT INTO tool_registry (scope, source, name, summary, description, schema_json, updated_at)
                 VALUES (?1, 'external', ?2, ?3, ?4, ?5, ?6)",
                params![scope, t.name, t.summary, t.description, t.schema_json, now],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn list_tool_summaries(
        &self,
        scope: &str,
        source: &str,
    ) -> Result<Vec<ToolRegistrySummary>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT name, summary FROM tool_registry WHERE scope = ?1 AND source = ?2 ORDER BY name ASC",
        )?;
        let rows = stmt.query_map(params![scope, source], |row| {
            Ok(ToolRegistrySummary {
                name: row.get(0)?,
                summary: row.get(1)?,
            })
        })?;
        rows.collect()
    }

    pub fn list_tools(
        &self,
        scope: &str,
        source: &str,
    ) -> Result<Vec<ToolRegistryTool>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT name, summary, description, schema_json FROM tool_registry WHERE scope = ?1 AND source = ?2 ORDER BY name ASC",
        )?;
        let rows = stmt.query_map(params![scope, source], |row| {
            Ok(ToolRegistryTool {
                name: row.get(0)?,
                summary: row.get(1)?,
                description: row.get(2)?,
                schema_json: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_tool(
        &self,
        scope: &str,
        source: &str,
        name: &str,
    ) -> Result<Option<ToolRegistryTool>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT name, summary, description, schema_json FROM tool_registry WHERE scope = ?1 AND source = ?2 AND name = ?3",
        )?;
        let row = stmt.query_row(params![scope, source, name], |row| {
            Ok(ToolRegistryTool {
                name: row.get(0)?,
                summary: row.get(1)?,
                description: row.get(2)?,
                schema_json: row.get(3)?,
            })
        });
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
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

    pub fn get_cached_user_prompt(&self, hash: &str) -> Result<Option<String>, rusqlite::Error> {
        let hash = hash.trim();
        if hash.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT prompt FROM user_prompt_cache WHERE hash = ?1")?;
        let result: Result<String, _> = stmt.query_row(params![hash], |row| row.get(0));
        match result {
            Ok(prompt) => Ok(Some(prompt)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn put_cached_user_prompt(&self, hash: &str, prompt: &str) -> Result<(), rusqlite::Error> {
        let hash = hash.trim();
        if hash.is_empty() {
            return Ok(());
        }
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        conn.execute(
            "INSERT INTO user_prompt_cache (hash, prompt, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(hash) DO UPDATE SET prompt = excluded.prompt, updated_at = excluded.updated_at",
            params![hash, prompt, now],
        )?;
        Ok(())
    }

    pub fn set_scope_user_prompt(&self, scope: &str, hash: &str) -> Result<(), rusqlite::Error> {
        let scope = scope.trim();
        if scope.is_empty() {
            return Ok(());
        }
        let hash = hash.trim();
        if hash.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        conn.execute(
            "INSERT INTO session_prompt (scope, prompt_hash, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(scope) DO UPDATE SET prompt_hash = excluded.prompt_hash, updated_at = excluded.updated_at",
            params![scope, hash, now],
        )?;
        Ok(())
    }

    pub fn get_scope_user_prompt(&self, scope: &str) -> Result<Option<String>, rusqlite::Error> {
        let scope = scope.trim();
        if scope.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT cache.prompt FROM session_prompt AS sp
             JOIN user_prompt_cache AS cache ON cache.hash = sp.prompt_hash
             WHERE sp.scope = ?1",
        )?;
        let result: Result<String, _> = stmt.query_row(params![scope], |row| row.get(0));
        match result {
            Ok(prompt) => Ok(Some(prompt)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    // Conclave self identity

    pub fn get_conclave_self(&self) -> Result<String, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT content FROM conclave_self WHERE id = 1")?;
        let result: Result<String, _> = stmt.query_row([], |row| row.get(0));
        Ok(result.unwrap_or_default())
    }

    pub fn set_conclave_self(&self, content: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        conn.execute(
            "INSERT INTO conclave_self (id, content, updated_at)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET content = ?1, updated_at = ?2",
            params![content, now],
        )?;
        Ok(())
    }

    // Conclave sessions

    pub fn save_conclave(
        &self,
        id: &str,
        status: &str,
        transcript: &str,
        decision: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        conn.execute(
            "INSERT OR REPLACE INTO conclaves (id, status, transcript, decision, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, status, transcript, decision, now],
        )?;
        Ok(())
    }

    pub fn list_conclaves(&self, limit: usize) -> Result<Vec<ConclaveRecord>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, status, transcript, decision, created_at
             FROM conclaves
             ORDER BY created_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(ConclaveRecord {
                id: row.get(0)?,
                status: row.get(1)?,
                transcript: row.get(2)?,
                decision: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_conclave(&self, id: &str) -> Result<Option<ConclaveRecord>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, status, transcript, decision, created_at
             FROM conclaves WHERE id = ?1",
        )?;
        let result = stmt.query_row(params![id], |row| {
            Ok(ConclaveRecord {
                id: row.get(0)?,
                status: row.get(1)?,
                transcript: row.get(2)?,
                decision: row.get(3)?,
                created_at: row.get(4)?,
            })
        });
        match result {
            Ok(c) => Ok(Some(c)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    // Wants pool management

    pub fn add_want(
        &self,
        id: &str,
        want: &str,
        context: &str,
        priority: &str,
        source: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        conn.execute(
            "INSERT INTO wants (id, want, context, priority, source, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, want, context, priority, source, now],
        )?;
        Ok(())
    }

    pub fn remove_want(&self, id: &str) -> Result<bool, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM wants WHERE id = ?1", params![id])?;
        Ok(rows > 0)
    }

    pub fn get_want(&self, id: &str) -> Result<Option<Want>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, want, context, priority, source, created_at FROM wants WHERE id = ?1",
        )?;
        let result = stmt.query_row(params![id], |row| {
            Ok(Want {
                id: row.get(0)?,
                want: row.get(1)?,
                context: row.get(2)?,
                priority: row.get(3)?,
                source: row.get(4)?,
                created_at: row.get(5)?,
            })
        });
        match result {
            Ok(w) => Ok(Some(w)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn list_wants(&self, limit: usize) -> Result<Vec<Want>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, want, context, priority, source, created_at
             FROM wants
             ORDER BY
                 CASE priority
                     WHEN 'urgent' THEN 0
                     WHEN 'high' THEN 1
                     WHEN 'normal' THEN 2
                     WHEN 'low' THEN 3
                     ELSE 4
                 END,
                 created_at ASC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(Want {
                id: row.get(0)?,
                want: row.get(1)?,
                context: row.get(2)?,
                priority: row.get(3)?,
                source: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn count_wants(&self) -> Result<usize, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM wants", [], |row| row.get(0))?;
        Ok(count as usize)
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

    pub fn log_llm_interaction(
        &self,
        agent: &str,
        run_id: &str,
        iter: usize,
        request_json: &str,
        response_json: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        conn.execute(
            "INSERT INTO llm_interaction (agent, run_id, iter, request_json, response_json, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![agent, run_id, iter as i64, request_json, response_json, now],
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
