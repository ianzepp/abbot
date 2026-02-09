use std::path::Path;
use uuid::Uuid;

use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqliteSynchronous};

pub struct Store {
    pool: SqlitePool,
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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

impl Store {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, sqlx::Error> {
        let path = path.as_ref();
        let is_memory = path.to_string_lossy() == ":memory:";

        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);

        // In-memory DBs: each connection gets its own DB, so limit to 1
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 4 })
            .connect_with(opts)
            .await?;

        let schema = "
            CREATE TABLE IF NOT EXISTS head_memory (
                head_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                content TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (head_id, kind)
            );
            CREATE TABLE IF NOT EXISTS hand_exec (
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
            );
            CREATE INDEX IF NOT EXISTS idx_hand_exec_task ON hand_exec(task_id, step ASC);
            CREATE TABLE IF NOT EXISTS llm_interaction (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent TEXT NOT NULL,
                run_id TEXT NOT NULL,
                iter INTEGER NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                request_json TEXT NOT NULL,
                response_json TEXT NOT NULL,
                timestamp INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_llm_interaction_run ON llm_interaction(agent, run_id, iter ASC);
            CREATE TABLE IF NOT EXISTS tool_registry (
                room TEXT NOT NULL,
                source TEXT NOT NULL,
                name TEXT NOT NULL,
                summary TEXT NOT NULL,
                description TEXT NOT NULL,
                schema_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (room, source, name)
            );
            CREATE INDEX IF NOT EXISTS idx_tool_registry_room ON tool_registry(room, source);
            CREATE TABLE IF NOT EXISTS room_state (
                room TEXT PRIMARY KEY,
                active_thread_id TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS room_env (
                room TEXT PRIMARY KEY,
                env_block TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS room_model (
                room TEXT PRIMARY KEY,
                model TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS user_prompt_cache (
                hash TEXT PRIMARY KEY,
                prompt TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS room_prompt (
                room TEXT PRIMARY KEY,
                prompt_hash TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY(prompt_hash) REFERENCES user_prompt_cache(hash)
            );
            CREATE TABLE IF NOT EXISTS room_schedules (
                id TEXT PRIMARY KEY,
                room_type TEXT NOT NULL,
                room TEXT NOT NULL DEFAULT 'main',
                status TEXT NOT NULL DEFAULT 'scheduled',
                run_after_ms INTEGER NOT NULL,
                reason TEXT NOT NULL DEFAULT 'scheduled',
                wake_mode TEXT NOT NULL DEFAULT 'normal',
                constraints_json TEXT NOT NULL DEFAULT '{}',
                context TEXT NOT NULL DEFAULT '',
                attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                room_id TEXT,
                created_at_ms INTEGER NOT NULL,
                started_at_ms INTEGER,
                finished_at_ms INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_room_schedules_due
                ON room_schedules(status, run_after_ms ASC);
        ";

        sqlx::raw_sql(schema).execute(&pool).await?;

        if cfg!(debug_assertions) {
            sqlx::raw_sql(
                "DELETE FROM tool_registry;
                 DELETE FROM room_state;
                 DELETE FROM room_env;
                 DELETE FROM room_model;
                 DELETE FROM room_prompt;
                 DELETE FROM user_prompt_cache;",
            )
            .execute(&pool)
            .await?;
        }

        Ok(Self { pool })
    }

    pub async fn set_room_env(&self, room: &str, env_block: &str) -> Result<(), sqlx::Error> {
        let room = room.trim();
        if room.is_empty() {
            return Ok(());
        }
        let env_block = env_block.trim();
        if env_block.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        sqlx::query(
            "INSERT INTO room_env (room, env_block, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(room) DO UPDATE SET env_block = ?2, updated_at = ?3",
        )
        .bind(room)
        .bind(env_block)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_room_env(&self, room: &str) -> Result<Option<String>, sqlx::Error> {
        let row = sqlx::query("SELECT env_block FROM room_env WHERE room = ?1")
            .bind(room)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => {
                let s: String = r.get(0);
                let s = s.trim().to_string();
                if s.is_empty() { Ok(None) } else { Ok(Some(s)) }
            }
            None => Ok(None),
        }
    }

    pub async fn set_room_model(&self, room: &str, model: &str) -> Result<(), sqlx::Error> {
        let room = room.trim();
        if room.is_empty() {
            return Ok(());
        }
        let model = model.trim();
        if model.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        sqlx::query(
            "INSERT INTO room_model (room, model, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(room) DO UPDATE SET model = ?2, updated_at = ?3",
        )
        .bind(room)
        .bind(model)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_room_model(&self, room: &str) -> Result<Option<String>, sqlx::Error> {
        let row = sqlx::query("SELECT model FROM room_model WHERE room = ?1")
            .bind(room)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => {
                let s: String = r.get(0);
                let s = s.trim().to_string();
                if s.is_empty() { Ok(None) } else { Ok(Some(s)) }
            }
            None => Ok(None),
        }
    }

    pub async fn clear_room_model(&self, room: &str) -> Result<(), sqlx::Error> {
        let room = room.trim();
        if room.is_empty() {
            return Ok(());
        }
        sqlx::query("DELETE FROM room_model WHERE room = ?1")
            .bind(room)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_active_thread(&self, room: &str, thread_id: Uuid) -> Result<(), sqlx::Error> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO room_state (room, active_thread_id, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(room) DO UPDATE SET active_thread_id = ?2, updated_at = ?3",
        )
        .bind(room)
        .bind(thread_id.to_string())
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_active_thread(&self, room: &str) -> Result<Option<Uuid>, sqlx::Error> {
        let row = sqlx::query("SELECT active_thread_id FROM room_state WHERE room = ?1")
            .bind(room)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => {
                let s: String = r.get(0);
                Ok(Uuid::parse_str(&s).ok())
            }
            None => Ok(None),
        }
    }

    pub async fn replace_external_tools(
        &self,
        room: &str,
        tools: &[ToolRegistryTool],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM tool_registry WHERE room = ?1 AND source = 'external'")
            .bind(room)
            .execute(&mut *tx)
            .await?;
        let now = now_ms();
        for t in tools {
            sqlx::query(
                "INSERT INTO tool_registry (room, source, name, summary, description, schema_json, updated_at)
                 VALUES (?1, 'external', ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(room).bind(&t.name).bind(&t.summary).bind(&t.description).bind(&t.schema_json).bind(now)
            .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_tool_summaries(
        &self,
        room: &str,
        source: &str,
    ) -> Result<Vec<ToolRegistrySummary>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT name, summary FROM tool_registry WHERE room = ?1 AND source = ?2 ORDER BY name ASC",
        ).bind(room).bind(source).fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|r| ToolRegistrySummary {
                name: r.get(0),
                summary: r.get(1),
            })
            .collect())
    }

    pub async fn list_tools(
        &self,
        room: &str,
        source: &str,
    ) -> Result<Vec<ToolRegistryTool>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT name, summary, description, schema_json FROM tool_registry WHERE room = ?1 AND source = ?2 ORDER BY name ASC",
        ).bind(room).bind(source).fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|r| ToolRegistryTool {
                name: r.get(0),
                summary: r.get(1),
                description: r.get(2),
                schema_json: r.get(3),
            })
            .collect())
    }

    pub async fn get_tool(
        &self,
        room: &str,
        source: &str,
        name: &str,
    ) -> Result<Option<ToolRegistryTool>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT name, summary, description, schema_json FROM tool_registry WHERE room = ?1 AND source = ?2 AND name = ?3",
        ).bind(room).bind(source).bind(name).fetch_optional(&self.pool).await?;
        Ok(row.map(|r| ToolRegistryTool {
            name: r.get(0),
            summary: r.get(1),
            description: r.get(2),
            schema_json: r.get(3),
        }))
    }

    pub async fn get_head_memory(&self, head_id: &str, kind: &str) -> Result<String, sqlx::Error> {
        let row = sqlx::query("SELECT content FROM head_memory WHERE head_id = ?1 AND kind = ?2")
            .bind(head_id)
            .bind(kind)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get::<String, _>(0)).unwrap_or_default())
    }

    pub async fn set_head_memory(
        &self,
        head_id: &str,
        kind: &str,
        content: &str,
    ) -> Result<(), sqlx::Error> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO head_memory (head_id, kind, content, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(head_id, kind) DO UPDATE SET content = ?3, updated_at = ?4",
        )
        .bind(head_id)
        .bind(kind)
        .bind(content)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_head_ltm(&self, head_id: &str) -> Result<String, sqlx::Error> {
        self.get_head_memory(head_id, "ltm").await
    }

    pub async fn get_cached_user_prompt(&self, hash: &str) -> Result<Option<String>, sqlx::Error> {
        let hash = hash.trim();
        if hash.is_empty() {
            return Ok(None);
        }
        let row = sqlx::query("SELECT prompt FROM user_prompt_cache WHERE hash = ?1")
            .bind(hash)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    pub async fn put_cached_user_prompt(
        &self,
        hash: &str,
        prompt: &str,
    ) -> Result<(), sqlx::Error> {
        let hash = hash.trim();
        if hash.is_empty() {
            return Ok(());
        }
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        sqlx::query(
            "INSERT INTO user_prompt_cache (hash, prompt, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(hash) DO UPDATE SET prompt = excluded.prompt, updated_at = excluded.updated_at",
        ).bind(hash).bind(prompt).bind(now)
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn set_room_user_prompt(&self, room: &str, hash: &str) -> Result<(), sqlx::Error> {
        let room = room.trim();
        if room.is_empty() {
            return Ok(());
        }
        let hash = hash.trim();
        if hash.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        sqlx::query(
            "INSERT INTO room_prompt (room, prompt_hash, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(room) DO UPDATE SET prompt_hash = excluded.prompt_hash, updated_at = excluded.updated_at",
        ).bind(room).bind(hash).bind(now)
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn get_room_user_prompt(&self, room: &str) -> Result<Option<String>, sqlx::Error> {
        let room = room.trim();
        if room.is_empty() {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT cache.prompt FROM room_prompt AS rp
             JOIN user_prompt_cache AS cache ON cache.hash = rp.prompt_hash
             WHERE rp.room = ?1",
        )
        .bind(room)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.get(0)))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn log_hand_exec(
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
    ) -> Result<(), sqlx::Error> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO hand_exec (task_id, hand_id, step, tool, args, output, success, duration_ms, hand_thought, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )
        .bind(task_id).bind(hand_id).bind(step as i64).bind(tool).bind(args)
        .bind(output).bind(success as i32).bind(duration_ms as i64).bind(hand_thought).bind(now)
        .execute(&self.pool).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn log_llm_interaction(
        &self,
        agent: &str,
        run_id: &str,
        iter: usize,
        input_tokens: u32,
        output_tokens: u32,
        request_json: &str,
        response_json: &str,
    ) -> Result<(), sqlx::Error> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO llm_interaction (agent, run_id, iter, input_tokens, output_tokens, request_json, response_json, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(agent).bind(run_id).bind(iter as i64).bind(input_tokens).bind(output_tokens).bind(request_json).bind(response_json).bind(now)
        .execute(&self.pool).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn insert_room_schedule(
        &self,
        id: &str,
        room_type: &str,
        room: &str,
        run_after_ms: i64,
        reason: &str,
        wake_mode: &str,
        constraints_json: &str,
        context: &str,
    ) -> Result<(), sqlx::Error> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO room_schedules (id, room_type, room, status, run_after_ms, reason, wake_mode, constraints_json, context, created_at_ms)
             VALUES (?1, ?2, ?3, 'scheduled', ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(id).bind(room_type).bind(room).bind(run_after_ms).bind(reason)
        .bind(wake_mode).bind(constraints_json).bind(context).bind(now)
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn claim_due_room_schedule(
        &self,
        now_ms: i64,
    ) -> Result<Option<RoomScheduleRow>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id FROM room_schedules WHERE status = 'scheduled' AND run_after_ms <= ?1 ORDER BY run_after_ms ASC LIMIT 1",
        ).bind(now_ms).fetch_optional(&self.pool).await?;
        let Some(found) = row else {
            return Ok(None);
        };
        let id: String = found.get(0);
        let result = sqlx::query(
            "UPDATE room_schedules SET status = 'running', started_at_ms = ?1, attempts = attempts + 1 WHERE id = ?2 AND status = 'scheduled'",
        ).bind(now_ms).bind(&id).execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_room_schedule(&id).await
    }

    pub async fn complete_room_schedule(
        &self,
        id: &str,
        result_status: &str,
        last_error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = now_ms();
        sqlx::query(
            "UPDATE room_schedules SET status = ?1, last_error = ?2, finished_at_ms = ?3 WHERE id = ?4",
        ).bind(result_status).bind(last_error).bind(now).bind(id)
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn cancel_room_schedule(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE room_schedules SET status = 'cancelled' WHERE id = ?1 AND status = 'scheduled'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn reschedule_room_schedule(
        &self,
        id: &str,
        new_run_after_ms: i64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE room_schedules SET run_after_ms = ?1, status = 'scheduled' WHERE id = ?2 AND status IN ('scheduled', 'running')",
        ).bind(new_run_after_ms).bind(id).execute(&self.pool).await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_room_schedules(
        &self,
        status_filter: Option<&str>,
        type_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RoomScheduleRow>, sqlx::Error> {
        let mut sql = String::from(
            "SELECT id, room_type, room, status, run_after_ms, reason, wake_mode, constraints_json, context, attempts, last_error, room_id, created_at_ms, started_at_ms, finished_at_ms FROM room_schedules WHERE 1=1",
        );
        let mut idx = 0;
        if status_filter.is_some() {
            idx += 1;
            sql.push_str(&format!(" AND status = ?{idx}"));
        }
        if type_filter.is_some() {
            idx += 1;
            sql.push_str(&format!(" AND room_type = ?{idx}"));
        }
        idx += 1;
        sql.push_str(&format!(" ORDER BY run_after_ms ASC LIMIT ?{idx}"));

        let mut q = sqlx::query(&sql);
        if let Some(s) = status_filter {
            q = q.bind(s.to_string());
        }
        if let Some(t) = type_filter {
            q = q.bind(t.to_string());
        }
        q = q.bind(limit as i64);
        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows.iter().map(row_to_room_schedule).collect())
    }

    pub async fn get_room_schedule(
        &self,
        id: &str,
    ) -> Result<Option<RoomScheduleRow>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, room_type, room, status, run_after_ms, reason, wake_mode, constraints_json, context, attempts, last_error, room_id, created_at_ms, started_at_ms, finished_at_ms FROM room_schedules WHERE id = ?1",
        ).bind(id).fetch_optional(&self.pool).await?;
        Ok(row.as_ref().map(row_to_room_schedule))
    }

    pub async fn get_hand_execs(&self, task_id: &str) -> Result<Vec<HandExec>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, task_id, hand_id, step, tool, args, output, success, hand_thought FROM hand_exec WHERE task_id = ?1 ORDER BY step ASC",
        ).bind(task_id).fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|r| HandExec {
                id: r.get::<i64, _>(0),
                task_id: r.get(1),
                hand_id: r.get(2),
                step: r.get::<i64, _>(3) as usize,
                tool: r.get(4),
                args: r.get(5),
                output: r.get(6),
                success: r.get::<i32, _>(7) != 0,
                hand_thought: r.get(8),
            })
            .collect())
    }
}

fn row_to_room_schedule(r: &sqlx::sqlite::SqliteRow) -> RoomScheduleRow {
    RoomScheduleRow {
        id: r.get(0),
        room_type: r.get(1),
        room: r.get(2),
        status: r.get(3),
        run_after_ms: r.get(4),
        reason: r.get(5),
        wake_mode: r.get(6),
        constraints_json: r.get(7),
        context: r.get(8),
        attempts: r.get(9),
        last_error: r.get(10),
        room_id: r.get(11),
        created_at_ms: r.get(12),
        started_at_ms: r.get(13),
        finished_at_ms: r.get(14),
    }
}

#[derive(Debug, Clone)]
pub struct RoomScheduleRow {
    pub id: String,
    pub room_type: String,
    pub room: String,
    pub status: String,
    pub run_after_ms: i64,
    pub reason: String,
    pub wake_mode: String,
    pub constraints_json: String,
    pub context: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub room_id: Option<String>,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
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
