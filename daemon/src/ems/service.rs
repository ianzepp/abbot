//! EMS Service - Entity Management System
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! EMS is a schema-flexible SQLite-backed entity store. It accepts arbitrary
//! JSON objects and evolves the backing schema to match observed keys.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Schema-on-write: Tables/columns are created lazily when data arrives
//! - All columns are TEXT: Avoids type mismatches; JSON handles rich types
//! - Nested structures are JSON-encoded: Objects/arrays serialize to strings
//!
//! TRADE-OFFS
//! ==========
//! - ALTER TABLE on every new column: Acceptable for low-cardinality schemas.
//!   If entities have highly dynamic shapes, consider a document-table design.
//! - TEXT columns everywhere: Loses SQLite type affinity benefits but gains
//!   flexibility. Query performance on large datasets may suffer.
//! - JSON encode/decode round-trip: Slight overhead, but keeps schema simple.
//!
//! SECURITY MODEL
//! ==============
//! - Identifiers (table/column names) are validated against a strict regex
//! - SQL injection via identifiers is prevented by allowlisting
//! - The query() method has coarse mutation blocking (keyword prefix check)
//! - Callers should treat query() as privileged; don't expose to untrusted input
//!
//! CONCURRENCY
//! ===========
//! EMS uses a Mutex-wrapped connection. WAL mode and busy_timeout mitigate
//! contention, but heavy concurrent writes will serialize at the mutex.

use std::path::Path;
use std::sync::{Arc, Mutex};

use regex::Regex;
use rusqlite::{params, params_from_iter, Connection};
use serde_json::{json, Value};
use uuid::Uuid;

use super::migrations;
use super::where_builder::build_where_clause;

// =============================================================================
// ERRORS
// =============================================================================
//
// Structured error types for EMS operations. Errors carry a code for
// programmatic handling and a message for human debugging.

#[derive(Debug)]
pub struct EmsError {
    pub code: String,
    pub message: String,
}

impl EmsError {
    pub fn invalid_identifier(name: &str) -> Self {
        Self {
            code: "E_INVALID_IDENTIFIER".to_string(),
            message: format!("invalid identifier: {}", name),
        }
    }

    pub fn db(msg: impl Into<String>) -> Self {
        Self {
            code: "E_DB".to_string(),
            message: msg.into(),
        }
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self {
            code: "E_FORBIDDEN".to_string(),
            message: msg.into(),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: "E_NOT_FOUND".to_string(),
            message: msg.into(),
        }
    }
}

impl std::fmt::Display for EmsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for EmsError {}

/// Thread-safe handle to an EMS instance.
///
/// WHY Arc<Mutex>: EMS may be accessed from multiple async tasks. The Mutex
/// serializes access to the SQLite connection (which is not thread-safe).
pub type EmsHandle = Arc<Mutex<EmsService>>;

// =============================================================================
// SERVICE
// =============================================================================
//
// The core EMS service. Provides CRUD operations on schema-flexible entities.
//
// LIFECYCLE
// ---------
// 1. open() - Create or open the database, apply pragmas
// 2. CRUD operations - insert/select/update/delete
// 3. Drop - Connection closes automatically
//
// INVARIANTS
// ----------
// INV-1: Every table has an "id" TEXT PRIMARY KEY column
// INV-2: All columns are TEXT (except id which is also TEXT)
// INV-3: Identifiers match ^[A-Za-z_][A-Za-z0-9_]*$ and don't start with sqlite_

pub struct EmsService {
    conn: Connection,
}

impl EmsService {
    /// Open (or create) the EMS SQLite database.
    ///
    /// WHY WAL: improves read/write behavior when EMS is used from multiple tasks
    /// via a serialized mutex, and makes lock contention less pathological.
    /// WHY busy_timeout: avoids immediate `SQLITE_BUSY` failures for brief conflicts.
    /// WHY foreign_keys: keeps schema evolution honest if/when EMS grows relations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, EmsError> {
        let path = path.as_ref();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| EmsError::db(format!("failed to create directory: {}", e)))?;
        }

        let mut conn = Connection::open(path)
            .map_err(|e| EmsError::db(format!("failed to open database: {}", e)))?;

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA foreign_keys = ON;",
        )
        .map_err(|e| EmsError::db(format!("failed to apply pragmas: {}", e)))?;

        migrations::apply(&mut conn)?;

        Ok(Self { conn })
    }

    pub fn handle(self) -> EmsHandle {
        Arc::new(Mutex::new(self))
    }

    /// Execute a restricted, read-only SQL query.
    ///
    /// WHY this exists: operational introspection/debug tooling occasionally needs
    /// ad-hoc queries that do not fit the structured EMS APIs.
    ///
    /// SECURITY NOTE: this is a coarse policy check (keyword prefix). It prevents
    /// obvious mutations but is not a complete SQL validator. Treat this method as
    /// privileged and avoid exposing it directly to untrusted input.
    pub fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<Value>, EmsError> {
        let upper = sql.trim().to_uppercase();
        let forbidden = [
            "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE",
        ];
        if forbidden.iter().any(|kw| upper.starts_with(kw)) {
            return Err(EmsError::forbidden("mutating SQL not allowed in ems_query"));
        }

        let bound: Vec<rusqlite::types::Value> = params.iter().map(json_to_sqlite).collect();

        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| EmsError::db(format!("prepare error: {}", e)))?;

        let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

        let rows = stmt
            .query_map(params_from_iter(bound.iter()), |row| {
                let mut obj = serde_json::Map::new();
                for (i, name) in col_names.iter().enumerate() {
                    let val: rusqlite::types::Value = row.get(i)?;
                    obj.insert(name.clone(), sqlite_to_json(val));
                }
                Ok(Value::Object(obj))
            })
            .map_err(|e| EmsError::db(format!("query error: {}", e)))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| EmsError::db(format!("row error: {}", e)))?);
        }

        Ok(out)
    }

    /// Insert an entity into a table.
    ///
    /// Creates the table if it doesn't exist. Adds columns for any new keys.
    /// Generates a UUID for "id" if not provided.
    ///
    /// WHY RETURNING *: We want to return the full inserted row including
    /// any server-generated values (id, defaults) without a second query.
    pub fn insert(&mut self, table: &str, values: &Value) -> Result<Value, EmsError> {
        validate_identifier(table)?;

        let obj = values
            .as_object()
            .ok_or_else(|| EmsError::db("values must be an object"))?;

        // -------------------------------------------------------------------------
        // PHASE 1: SCHEMA EVOLUTION
        // Ensure table exists with all required columns
        // -------------------------------------------------------------------------
        self.ensure_table(table, obj)?;

        // -------------------------------------------------------------------------
        // PHASE 2: BUILD INSERT STATEMENT
        // Collect columns, placeholders, and bound values
        // -------------------------------------------------------------------------
        let mut has_id = false;
        let mut cols = Vec::new();
        let mut placeholders = Vec::new();
        let mut bound = Vec::new();

        for (k, v) in obj {
            validate_identifier(k)?;
            if k == "id" {
                has_id = true;
            }
            cols.push(format!("\"{}\"", k));
            placeholders.push("?");
            bound.push(json_to_sqlite(&encode_value(v)));
        }

        // Generate UUID if no id provided
        if !has_id {
            cols.push("\"id\"".to_string());
            placeholders.push("?");
            bound.push(rusqlite::types::Value::Text(Uuid::new_v4().to_string()));
        }

        let sql = format!(
            "INSERT INTO \"{}\" ({}) VALUES ({}) RETURNING *",
            table,
            cols.join(", "),
            placeholders.join(", ")
        );

        // -------------------------------------------------------------------------
        // PHASE 3: EXECUTE AND RETURN
        // Run the insert, decode the returned row
        // -------------------------------------------------------------------------
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| EmsError::db(format!("prepare error: {}", e)))?;

        let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

        let mut rows = stmt
            .query(params_from_iter(bound.iter()))
            .map_err(|e| EmsError::db(format!("insert error: {}", e)))?;

        if let Some(row) = rows.next().map_err(|e| EmsError::db(e.to_string()))? {
            let mut obj = serde_json::Map::new();
            for (i, name) in col_names.iter().enumerate() {
                let val: rusqlite::types::Value =
                    row.get(i).map_err(|e| EmsError::db(e.to_string()))?;
                obj.insert(name.clone(), decode_value(sqlite_to_json(val)));
            }
            Ok(Value::Object(obj))
        } else {
            Err(EmsError::db("insert returned no rows"))
        }
    }

    /// Select entities from a table with optional filtering, ordering, and pagination.
    ///
    /// WHERE CLAUSE FORMAT
    /// -------------------
    /// The where_clause uses a MongoDB-inspired syntax:
    /// - { "field": "value" } - equality
    /// - { "field": { "$gt": 10 } } - comparison operators
    /// - { "$or": [...] } - logical operators
    ///
    /// See where_builder.rs for full syntax documentation.
    pub fn select(
        &self,
        table: &str,
        where_clause: Option<&Value>,
        columns: Option<&[String]>,
        order_by: Option<&Value>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<Value>, EmsError> {
        validate_identifier(table)?;

        let cols_sql = if let Some(cols) = columns {
            let mut out = Vec::new();
            for c in cols {
                validate_identifier(c)?;
                out.push(format!("\"{}\"", c));
            }
            out.join(", ")
        } else {
            "*".to_string()
        };

        let mut sql = format!("SELECT {} FROM \"{}\"", cols_sql, table);
        let mut bound: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(w) = where_clause {
            let (clause, params) = build_where_clause(w)?;
            if !clause.is_empty() {
                sql.push_str(" WHERE ");
                sql.push_str(&clause);
                bound.extend(params.into_iter().map(|v| json_to_sqlite(&v)));
            }
        }

        if let Some(ob) = order_by {
            let order_sql = parse_order_by(ob)?;
            if !order_sql.is_empty() {
                sql.push_str(" ORDER BY ");
                sql.push_str(&order_sql);
            }
        }

        if let Some(lim) = limit {
            sql.push_str(&format!(" LIMIT {}", lim));
        }

        if let Some(off) = offset {
            sql.push_str(&format!(" OFFSET {}", off));
        }

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| EmsError::db(format!("prepare error: {}", e)))?;

        let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

        let rows = stmt
            .query_map(params_from_iter(bound.iter()), |row| {
                let mut obj = serde_json::Map::new();
                for (i, name) in col_names.iter().enumerate() {
                    let val: rusqlite::types::Value = row.get(i)?;
                    obj.insert(name.clone(), decode_value(sqlite_to_json(val)));
                }
                Ok(Value::Object(obj))
            })
            .map_err(|e| EmsError::db(format!("select error: {}", e)))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| EmsError::db(format!("row error: {}", e)))?);
        }

        Ok(out)
    }

    /// Update entities matching a where clause.
    ///
    /// Returns the number of rows affected.
    ///
    /// SAFETY: Requires a non-empty where clause to prevent accidental bulk updates.
    /// If you genuinely need to update all rows, use { "id": { "$ne": null } }.
    pub fn update(
        &mut self,
        table: &str,
        where_clause: &Value,
        changes: &Value,
    ) -> Result<usize, EmsError> {
        validate_identifier(table)?;

        let changes_obj = changes
            .as_object()
            .ok_or_else(|| EmsError::db("changes must be an object"))?;

        if changes_obj.is_empty() {
            return Err(EmsError::db("changes cannot be empty"));
        }

        let mut set_parts = Vec::new();
        let mut bound: Vec<rusqlite::types::Value> = Vec::new();

        for (k, v) in changes_obj {
            validate_identifier(k)?;
            set_parts.push(format!("\"{}\" = ?", k));
            bound.push(json_to_sqlite(&encode_value(v)));
        }

        let (where_sql, where_params) = build_where_clause(where_clause)?;
        if where_sql.is_empty() {
            return Err(EmsError::db("where clause is required for update"));
        }

        bound.extend(where_params.into_iter().map(|v| json_to_sqlite(&v)));

        let sql = format!(
            "UPDATE \"{}\" SET {} WHERE {}",
            table,
            set_parts.join(", "),
            where_sql
        );

        let changes = self
            .conn
            .execute(&sql, params_from_iter(bound.iter()))
            .map_err(|e| EmsError::db(format!("update error: {}", e)))?;

        Ok(changes)
    }

    /// Delete entities by ID.
    ///
    /// WHY by-ID only: Deletes are destructive. Requiring explicit IDs prevents
    /// accidental bulk deletion. For bulk operations, select IDs first, review,
    /// then delete.
    pub fn delete(&mut self, table: &str, ids: &[String]) -> Result<usize, EmsError> {
        validate_identifier(table)?;

        if ids.is_empty() {
            return Ok(0);
        }

        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let sql = format!(
            "DELETE FROM \"{}\" WHERE id IN ({})",
            table,
            placeholders.join(", ")
        );

        let bound: Vec<rusqlite::types::Value> = ids
            .iter()
            .map(|id| rusqlite::types::Value::Text(id.clone()))
            .collect();

        let changes = self
            .conn
            .execute(&sql, params_from_iter(bound.iter()))
            .map_err(|e| EmsError::db(format!("delete error: {}", e)))?;

        Ok(changes)
    }

    /// Atomically claim one row matching a WHERE clause, apply changes, and return it.
    ///
    /// Used for lease-style operations: find the next eligible row, update it
    /// (e.g. status → "running"), and return the updated row — all under the
    /// existing Mutex so no other caller can claim the same row.
    ///
    /// `order_by_raw` is a trusted SQL ORDER BY fragment (e.g. "priority_rank ASC, created_at ASC").
    /// Only called from kernel syscall code, never from LLM tools.
    pub fn claim_one(
        &mut self,
        table: &str,
        where_clause: &Value,
        order_by_raw: &str,
        changes: &Value,
    ) -> Result<Option<Value>, EmsError> {
        validate_identifier(table)?;

        // Lightweight validation of order_by_raw — reject obvious injection patterns.
        if order_by_raw.contains(';')
            || order_by_raw.contains("--")
            || order_by_raw.to_uppercase().contains("DROP")
            || order_by_raw.to_uppercase().contains("DELETE")
            || order_by_raw.to_uppercase().contains("INSERT")
            || order_by_raw.to_uppercase().contains("UPDATE")
        {
            return Err(EmsError::forbidden("invalid order_by_raw fragment"));
        }

        // Phase 1: Find candidate row ID
        let (where_sql, where_params) = build_where_clause(where_clause)?;
        if where_sql.is_empty() {
            return Err(EmsError::db("where clause is required for claim_one"));
        }

        let bound: Vec<rusqlite::types::Value> =
            where_params.into_iter().map(|v| json_to_sqlite(&v)).collect();

        let find_sql = format!(
            "SELECT \"id\" FROM \"{}\" WHERE {} ORDER BY {} LIMIT 1",
            table, where_sql, order_by_raw
        );

        let row_id: Option<String> = self
            .conn
            .query_row(&find_sql, params_from_iter(bound.iter()), |row| row.get(0))
            .ok();

        let Some(row_id) = row_id else {
            return Ok(None);
        };

        // Phase 2: Apply changes
        let changes_obj = changes
            .as_object()
            .ok_or_else(|| EmsError::db("changes must be an object"))?;

        if changes_obj.is_empty() {
            return Err(EmsError::db("changes cannot be empty"));
        }

        // Ensure table has columns for any new keys in changes
        self.ensure_table(table, changes_obj)?;

        let mut set_parts = Vec::new();
        let mut update_bound: Vec<rusqlite::types::Value> = Vec::new();

        for (k, v) in changes_obj {
            validate_identifier(k)?;
            set_parts.push(format!("\"{}\" = ?", k));
            update_bound.push(json_to_sqlite(&encode_value(v)));
        }

        update_bound.push(rusqlite::types::Value::Text(row_id.clone()));

        let update_sql = format!(
            "UPDATE \"{}\" SET {} WHERE \"id\" = ?",
            table,
            set_parts.join(", ")
        );

        self.conn
            .execute(&update_sql, params_from_iter(update_bound.iter()))
            .map_err(|e| EmsError::db(format!("claim_one update error: {}", e)))?;

        // Phase 3: Return updated row
        let select_sql = format!("SELECT * FROM \"{}\" WHERE \"id\" = ?", table);
        let mut stmt = self
            .conn
            .prepare(&select_sql)
            .map_err(|e| EmsError::db(format!("prepare error: {}", e)))?;

        let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

        let row = stmt
            .query_row(params![row_id], |row| {
                let mut obj = serde_json::Map::new();
                for (i, name) in col_names.iter().enumerate() {
                    let val: rusqlite::types::Value = row.get(i)?;
                    obj.insert(name.clone(), decode_value(sqlite_to_json(val)));
                }
                Ok(Value::Object(obj))
            })
            .map_err(|e| EmsError::db(format!("claim_one select error: {}", e)))?;

        Ok(Some(row))
    }

    /// Describe the schema: list tables, or describe a specific table's columns.
    ///
    /// With table=None: Returns { "tables": ["name1", "name2", ...] }
    /// With table=Some: Returns { "table": "name", "columns": [...] }
    pub fn describe(&self, table: Option<&str>) -> Result<Value, EmsError> {
        if let Some(t) = table {
            validate_identifier(t)?;

            let sql = format!("PRAGMA table_info(\"{}\")", t);
            let mut stmt = self
                .conn
                .prepare(&sql)
                .map_err(|e| EmsError::db(format!("pragma error: {}", e)))?;

            let columns = stmt
                .query_map([], |row| {
                    let name: String = row.get(1)?;
                    let col_type: String = row.get(2)?;
                    let notnull: i32 = row.get(3)?;
                    let pk: i32 = row.get(5)?;
                    Ok(json!({
                        "name": name,
                        "type": col_type,
                        "notnull": notnull != 0,
                        "pk": pk != 0
                    }))
                })
                .map_err(|e| EmsError::db(e.to_string()))?;

            let mut out = Vec::new();
            for col in columns {
                out.push(col.map_err(|e| EmsError::db(e.to_string()))?);
            }

            if out.is_empty() {
                return Err(EmsError::not_found(format!("table not found: {}", t)));
            }

            Ok(json!({
                "table": t,
                "columns": out
            }))
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
            )
            .map_err(|e| EmsError::db(format!("query error: {}", e)))?;

            let tables = stmt
                .query_map([], |row| {
                    let name: String = row.get(0)?;
                    Ok(name)
                })
                .map_err(|e| EmsError::db(e.to_string()))?;

            let mut out = Vec::new();
            for t in tables {
                out.push(t.map_err(|e| EmsError::db(e.to_string()))?);
            }

            Ok(json!({
                "tables": out
            }))
        }
    }

    /// Ensure a table exists with columns for all provided keys.
    ///
    /// SCHEMA EVOLUTION STRATEGY
    /// -------------------------
    /// EMS is schema-flexible. We accept JSON objects and evolve the backing
    /// schema to match observed keys. This method:
    /// 1. Creates the table if it doesn't exist
    /// 2. Adds columns for any new keys
    ///
    /// TRADE-OFF: ALTER TABLE is not free. If entities have highly dynamic
    /// shapes (thousands of unique keys), consider a document-table design
    /// with a single JSON column instead.
    fn ensure_table(
        &mut self,
        table: &str,
        values: &serde_json::Map<String, Value>,
    ) -> Result<(), EmsError> {
        // -------------------------------------------------------------------------
        // CHECK: Does table exist?
        // -------------------------------------------------------------------------
        let exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
                params![table],
                |_| Ok(true),
            )
            .unwrap_or(false);

        // -------------------------------------------------------------------------
        // PATH A: Create new table
        // WHY TEXT columns: Avoids type-mismatch failures for loosely-typed JSON
        // inputs and keeps schema evolution simple.
        // -------------------------------------------------------------------------
        if !exists {
            let mut col_defs = vec!["\"id\" TEXT PRIMARY KEY".to_string()];
            for k in values.keys() {
                if k == "id" {
                    continue;
                }
                validate_identifier(k)?;
                col_defs.push(format!("\"{}\" TEXT", k));
            }
            let sql = format!("CREATE TABLE \"{}\" ({})", table, col_defs.join(", "));
            self.conn
                .execute(&sql, [])
                .map_err(|e| EmsError::db(format!("create table error: {}", e)))?;
            return Ok(());
        }

        // -------------------------------------------------------------------------
        // PATH B: Add missing columns to existing table
        // -------------------------------------------------------------------------
        let mut stmt = self
            .conn
            .prepare(&format!("PRAGMA table_info(\"{}\")", table))
            .map_err(|e| EmsError::db(e.to_string()))?;

        let existing_cols: std::collections::HashSet<String> = stmt
            .query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(name)
            })
            .map_err(|e| EmsError::db(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        for k in values.keys() {
            if existing_cols.contains(k) {
                continue;
            }
            validate_identifier(k)?;
            let sql = format!("ALTER TABLE \"{}\" ADD COLUMN \"{}\" TEXT", table, k);
            self.conn
                .execute(&sql, [])
                .map_err(|e| EmsError::db(format!("alter table error: {}", e)))?;
        }

        Ok(())
    }
}

// =============================================================================
// IDENTIFIERS AND VALUE ENCODING
// =============================================================================
//
// These helpers handle the boundary between JSON values and SQLite storage.
// Key challenges:
// - Identifiers can't be parameterized in SQLite, must be validated
// - Nested JSON must round-trip through TEXT columns
// - Type coercion between JSON and SQLite value systems

/// Validate EMS identifiers before quoting/interpolating into SQL.
///
/// WHY: Identifiers (table/column names) cannot be bound as parameters in SQLite.
/// We validate them to a conservative subset to avoid SQL injection via identifiers.
fn validate_identifier(name: &str) -> Result<(), EmsError> {
    let re = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap();
    if !re.is_match(name) || name.to_lowercase().starts_with("sqlite_") {
        return Err(EmsError::invalid_identifier(name));
    }
    Ok(())
}

/// Encode values for storage in TEXT columns.
///
/// WHY: Objects/arrays are serialized to JSON strings so the schema can remain
/// stable (TEXT) while still supporting nested structures.
fn encode_value(v: &Value) -> Value {
    match v {
        Value::Object(_) | Value::Array(_) => Value::String(v.to_string()),
        _ => v.clone(),
    }
}

/// Decode stored values back into JSON when it is unambiguous.
///
/// TRADE-OFF: This is best-effort and can misinterpret user strings that happen
/// to look like JSON. If strict typing becomes important, store a type tag.
fn decode_value(v: Value) -> Value {
    if let Value::String(s) = &v {
        let trimmed = s.trim();
        if (trimmed.starts_with('{') && trimmed.ends_with('}'))
            || (trimmed.starts_with('[') && trimmed.ends_with(']'))
        {
            if let Ok(parsed) = serde_json::from_str(trimmed) {
                return parsed;
            }
        }
    }
    v
}

/// Convert a JSON value to a SQLite value for parameter binding.
///
/// TYPE MAPPING:
/// - null -> NULL
/// - bool -> INTEGER (0/1)
/// - number -> INTEGER or REAL
/// - string -> TEXT
/// - array/object -> TEXT (JSON-serialized)
fn json_to_sqlite(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(b) => rusqlite::types::Value::Integer(if *b { 1 } else { 0 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rusqlite::types::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                rusqlite::types::Value::Real(f)
            } else {
                rusqlite::types::Value::Text(n.to_string())
            }
        }
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        Value::Array(_) | Value::Object(_) => rusqlite::types::Value::Text(v.to_string()),
    }
}

/// Convert a SQLite value back to JSON.
///
/// WHY base64 for blobs: JSON has no binary type. Base64 is universally
/// decodable and safe for transport.
fn sqlite_to_json(v: rusqlite::types::Value) -> Value {
    match v {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(i) => json!(i),
        rusqlite::types::Value::Real(f) => json!(f),
        rusqlite::types::Value::Text(s) => Value::String(s),
        rusqlite::types::Value::Blob(b) => Value::String(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &b,
        )),
    }
}

// =============================================================================
// QUERY HELPERS
// =============================================================================
//
// SQL generation utilities. These translate EMS API conventions into safe SQL.

/// Parse EMS order_by into a safe `ORDER BY` clause.
///
/// WHY: callers may supply ordering dynamically; we validate identifiers and only
/// accept `ASC`/`DESC` direction to prevent SQL injection.
fn parse_order_by(v: &Value) -> Result<String, EmsError> {
    match v {
        Value::String(s) => {
            let parts: Vec<&str> = s.split_whitespace().collect();
            if parts.is_empty() {
                return Ok(String::new());
            }
            validate_identifier(parts[0])?;
            let dir = if parts.len() > 1 {
                let d = parts[1].to_uppercase();
                if d == "DESC" {
                    "DESC"
                } else {
                    "ASC"
                }
            } else {
                "ASC"
            };
            Ok(format!("\"{}\" {}", parts[0], dir))
        }
        Value::Array(arr) => {
            let mut parts = Vec::new();
            for item in arr {
                if let Value::String(s) = item {
                    let cols: Vec<&str> = s.split_whitespace().collect();
                    if cols.is_empty() {
                        continue;
                    }
                    validate_identifier(cols[0])?;
                    let dir = if cols.len() > 1 {
                        let d = cols[1].to_uppercase();
                        if d == "DESC" {
                            "DESC"
                        } else {
                            "ASC"
                        }
                    } else {
                        "ASC"
                    };
                    parts.push(format!("\"{}\" {}", cols[0], dir));
                }
            }
            Ok(parts.join(", "))
        }
        _ => Ok(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_identifier() {
        assert!(validate_identifier("users").is_ok());
        assert!(validate_identifier("user_id").is_ok());
        assert!(validate_identifier("_private").is_ok());
        assert!(validate_identifier("Col123").is_ok());

        assert!(validate_identifier("123start").is_err());
        assert!(validate_identifier("has-dash").is_err());
        assert!(validate_identifier("has.dot").is_err());
        assert!(validate_identifier("sqlite_master").is_err());
        assert!(validate_identifier("SQLITE_sequence").is_err());
    }

    #[test]
    fn test_encode_decode_value() {
        let obj = json!({"nested": "value"});
        let encoded = encode_value(&obj);
        assert!(encoded.is_string());

        let decoded = decode_value(encoded);
        assert_eq!(decoded, obj);

        let plain = json!("plain string");
        assert_eq!(encode_value(&plain), plain);
        assert_eq!(decode_value(plain.clone()), plain);
    }

    #[test]
    fn test_parse_order_by() {
        assert_eq!(
            parse_order_by(&json!("created_at")).unwrap(),
            "\"created_at\" ASC"
        );
        assert_eq!(
            parse_order_by(&json!("created_at DESC")).unwrap(),
            "\"created_at\" DESC"
        );
        assert_eq!(
            parse_order_by(&json!(["status", "created_at DESC"])).unwrap(),
            "\"status\" ASC, \"created_at\" DESC"
        );
    }
}
