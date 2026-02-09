//! EMS Service - Entity Management System
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! EMS is a unified SQLite-backed entity store. All entities live in a single
//! `entities` table with a `kind` column. Callers pass a logical table name
//! (e.g. "tasks", "needs", "wants") which is transparently mapped to
//! `WHERE kind = ?` on the `entities` table.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Single table: All entities share a fixed schema with a JSON `data` blob
//! - Kind-mapped: Callers use logical table names, service maps to kind filter
//! - Fixed columns: id, kind, status, priority, room, prompt, created_at, updated_at
//! - Overflow to data: Non-fixed columns are packed into a JSON `data` blob
//! - On read: data blob is unpacked and merged into the returned object
//!
//! SECURITY MODEL
//! ==============
//! - Identifiers (column names in changes/where) are validated against a strict regex
//! - The query() method has coarse mutation blocking (keyword prefix check)
//! - Callers should treat query() as privileged; don't expose to untrusted input

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{Column, Row};
use uuid::Uuid;

use super::migrations;
use super::where_builder::build_where_clause;

// =============================================================================
// CONSTANTS
// =============================================================================

/// Columns that exist as real columns in the `entities` table.
const FIXED_COLUMNS: &[&str] = &[
    "id",
    "kind",
    "status",
    "priority",
    "room",
    "prompt",
    "created_at",
    "updated_at",
    "data",
];

fn is_fixed_column(col: &str) -> bool {
    FIXED_COLUMNS.contains(&col)
}

// =============================================================================
// ERRORS
// =============================================================================

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
pub type EmsHandle = Arc<tokio::sync::Mutex<EmsService>>;

// =============================================================================
// SERVICE
// =============================================================================

pub struct EmsService {
    pool: SqlitePool,
}

impl EmsService {
    /// Open (or create) the EMS SQLite database.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, EmsError> {
        let path = path.as_ref();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| EmsError::db(format!("failed to create directory: {}", e)))?;
        }

        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .map_err(|e| EmsError::db(format!("failed to open database: {}", e)))?;

        migrations::apply(&pool).await?;

        Ok(Self { pool })
    }

    pub fn handle(self) -> EmsHandle {
        Arc::new(tokio::sync::Mutex::new(self))
    }

    /// Execute a restricted, read-only SQL query.
    pub async fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<Value>, EmsError> {
        let upper = sql.trim().to_uppercase();
        let forbidden = [
            "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE",
        ];
        if forbidden.iter().any(|kw| upper.starts_with(kw)) {
            return Err(EmsError::forbidden("mutating SQL not allowed in ems_query"));
        }

        let mut q = sqlx::query(sql);
        for val in params {
            q = bind_json_value(q, val);
        }

        let rows = q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| EmsError::db(format!("query error: {}", e)))?;

        let mut out = Vec::new();
        for row in &rows {
            let col_names: Vec<String> =
                row.columns().iter().map(|c| c.name().to_string()).collect();
            let mut obj = serde_json::Map::new();
            for (i, name) in col_names.iter().enumerate() {
                obj.insert(name.clone(), read_column_as_json(row, i));
            }
            out.push(Value::Object(obj));
        }

        Ok(out)
    }

    /// Insert an entity into the unified `entities` table.
    ///
    /// The `table` parameter becomes the `kind` value (e.g. "tasks" → "task",
    /// "needs" → "need", "wants" → "want"). Fixed columns go to real columns;
    /// everything else is packed into the `data` JSON blob.
    pub async fn insert(&mut self, table: &str, values: &Value) -> Result<Value, EmsError> {
        let kind = table_to_kind(table);

        let obj = values
            .as_object()
            .ok_or_else(|| EmsError::db("values must be an object"))?;

        // Separate fixed columns from extras
        let (fixed, extras) = partition_columns(obj);

        let mut cols = Vec::new();
        let mut placeholders = Vec::new();
        let mut bound = Vec::new();

        // Always set kind
        cols.push("\"kind\"".to_string());
        placeholders.push("?");
        bound.push(Value::String(kind.to_string()));

        // Add fixed columns
        let mut has_id = false;
        for (k, v) in &fixed {
            if k == "kind" {
                continue; // already added
            }
            if k == "id" {
                has_id = true;
            }
            cols.push(format!("\"{}\"", k));
            placeholders.push("?");
            bound.push(encode_value(v));
        }

        // Generate UUID if no id provided
        if !has_id {
            cols.push("\"id\"".to_string());
            placeholders.push("?");
            bound.push(Value::String(Uuid::new_v4().to_string()));
        }

        // Pack extras into data blob
        if !extras.is_empty() {
            cols.push("\"data\"".to_string());
            placeholders.push("?");
            let data_blob = serde_json::to_string(&Value::Object(extras.into_iter().collect()))
                .unwrap_or_else(|_| "{}".to_string());
            bound.push(Value::String(data_blob));
        }

        let sql = format!(
            "INSERT INTO \"entities\" ({}) VALUES ({}) RETURNING *",
            cols.join(", "),
            placeholders.join(", ")
        );

        let mut q = sqlx::query(&sql);
        for val in &bound {
            q = bind_json_value(q, val);
        }

        let row = q
            .fetch_one(&self.pool)
            .await
            .map_err(|e| EmsError::db(format!("insert error: {}", e)))?;

        Ok(unpack_row(&row))
    }

    /// Select entities from the unified table, filtered by kind.
    pub async fn select(
        &self,
        table: &str,
        where_clause: Option<&Value>,
        columns: Option<&[String]>,
        order_by: Option<&Value>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<Value>, EmsError> {
        let kind = table_to_kind(table);

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

        let mut sql = format!("SELECT {} FROM \"entities\"", cols_sql);
        let mut bound: Vec<Value> = Vec::new();

        // Always filter by kind
        sql.push_str(" WHERE \"kind\" = ?");
        bound.push(Value::String(kind.to_string()));

        if let Some(w) = where_clause {
            let (clause, params) = build_where_clause(w)?;
            if !clause.is_empty() {
                sql.push_str(" AND ");
                sql.push_str(&clause);
                bound.extend(params);
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

        let mut q = sqlx::query(&sql);
        for val in &bound {
            q = bind_json_value(q, val);
        }

        let rows = q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| EmsError::db(format!("select error: {}", e)))?;

        let mut out = Vec::new();
        for row in &rows {
            out.push(unpack_row(row));
        }

        Ok(out)
    }

    /// Update entities matching a where clause, scoped to kind.
    pub async fn update(
        &mut self,
        table: &str,
        where_clause: &Value,
        changes: &Value,
    ) -> Result<usize, EmsError> {
        let kind = table_to_kind(table);

        let changes_obj = changes
            .as_object()
            .ok_or_else(|| EmsError::db("changes must be an object"))?;

        if changes_obj.is_empty() {
            return Err(EmsError::db("changes cannot be empty"));
        }

        // Separate fixed from extras in changes
        let (fixed_changes, extra_changes) = partition_columns(changes_obj);

        let mut set_parts = Vec::new();
        let mut bound: Vec<Value> = Vec::new();

        // SET fixed columns directly
        for (k, v) in &fixed_changes {
            if k == "kind" || k == "id" {
                continue; // never update kind or id
            }
            set_parts.push(format!("\"{}\" = ?", k));
            bound.push(encode_value(v));
        }

        // Merge extras into data blob via json_patch
        if !extra_changes.is_empty() {
            set_parts.push("\"data\" = json_patch(\"data\", ?)".to_string());
            let patch = serde_json::to_string(&Value::Object(extra_changes.into_iter().collect()))
                .unwrap_or_else(|_| "{}".to_string());
            bound.push(Value::String(patch));
        }

        if set_parts.is_empty() {
            return Err(EmsError::db("no updatable columns in changes"));
        }

        // Build WHERE clause with kind filter
        let (where_sql, where_params) = build_where_clause(where_clause)?;
        if where_sql.is_empty() {
            return Err(EmsError::db("where clause is required for update"));
        }

        bound.extend(where_params);
        bound.push(Value::String(kind.to_string()));

        let sql = format!(
            "UPDATE \"entities\" SET {} WHERE {} AND \"kind\" = ?",
            set_parts.join(", "),
            where_sql
        );

        let mut q = sqlx::query(&sql);
        for val in &bound {
            q = bind_json_value(q, val);
        }

        let result = q
            .execute(&self.pool)
            .await
            .map_err(|e| EmsError::db(format!("update error: {}", e)))?;

        Ok(result.rows_affected() as usize)
    }

    /// Delete entities by ID, scoped to kind.
    pub async fn delete(&mut self, table: &str, ids: &[String]) -> Result<usize, EmsError> {
        let kind = table_to_kind(table);

        if ids.is_empty() {
            return Ok(0);
        }

        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let sql = format!(
            "DELETE FROM \"entities\" WHERE \"kind\" = ? AND id IN ({})",
            placeholders.join(", ")
        );

        let mut q = sqlx::query(&sql);
        q = q.bind(kind);
        for id in ids {
            q = q.bind(id.as_str());
        }

        let result = q
            .execute(&self.pool)
            .await
            .map_err(|e| EmsError::db(format!("delete error: {}", e)))?;

        Ok(result.rows_affected() as usize)
    }

    /// Atomically claim one row matching a WHERE clause, apply changes, and return it.
    pub async fn claim_one(
        &mut self,
        table: &str,
        where_clause: &Value,
        order_by_raw: &str,
        changes: &Value,
    ) -> Result<Option<Value>, EmsError> {
        let kind = table_to_kind(table);

        // Lightweight validation of order_by_raw
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

        let find_sql = format!(
            "SELECT \"id\" FROM \"entities\" WHERE \"kind\" = ? AND {} ORDER BY {} LIMIT 1",
            where_sql, order_by_raw
        );

        let mut q = sqlx::query_scalar::<_, String>(&find_sql);
        q = q.bind(kind);
        for val in &where_params {
            q = bind_json_value_scalar(q, val);
        }

        let row_id: Option<String> = q
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| EmsError::db(format!("claim_one find error: {}", e)))?;

        let Some(row_id) = row_id else {
            return Ok(None);
        };

        // Phase 2: Apply changes (use update method logic inline)
        let changes_obj = changes
            .as_object()
            .ok_or_else(|| EmsError::db("changes must be an object"))?;

        if changes_obj.is_empty() {
            return Err(EmsError::db("changes cannot be empty"));
        }

        let (fixed_changes, extra_changes) = partition_columns(changes_obj);

        let mut set_parts = Vec::new();
        let mut update_bound: Vec<Value> = Vec::new();

        for (k, v) in &fixed_changes {
            if k == "kind" || k == "id" {
                continue;
            }
            set_parts.push(format!("\"{}\" = ?", k));
            update_bound.push(encode_value(v));
        }

        if !extra_changes.is_empty() {
            set_parts.push("\"data\" = json_patch(\"data\", ?)".to_string());
            let patch = serde_json::to_string(&Value::Object(extra_changes.into_iter().collect()))
                .unwrap_or_else(|_| "{}".to_string());
            update_bound.push(Value::String(patch));
        }

        if set_parts.is_empty() {
            return Err(EmsError::db("no updatable columns in changes"));
        }

        update_bound.push(Value::String(row_id.clone()));

        let update_sql = format!(
            "UPDATE \"entities\" SET {} WHERE \"id\" = ?",
            set_parts.join(", ")
        );

        let mut q = sqlx::query(&update_sql);
        for val in &update_bound {
            q = bind_json_value(q, val);
        }
        q.execute(&self.pool)
            .await
            .map_err(|e| EmsError::db(format!("claim_one update error: {}", e)))?;

        // Phase 3: Return updated row
        let select_sql = "SELECT * FROM \"entities\" WHERE \"id\" = ?";
        let row = sqlx::query(select_sql)
            .bind(&row_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| EmsError::db(format!("claim_one select error: {}", e)))?;

        Ok(Some(unpack_row(&row)))
    }

    /// Describe the schema: list kinds, or describe the entities table columns.
    pub async fn describe(&self, table: Option<&str>) -> Result<Value, EmsError> {
        if let Some(_t) = table {
            // Return fixed column info for the entities table
            let cols: Vec<Value> = FIXED_COLUMNS
                .iter()
                .map(|&name| {
                    json!({
                        "name": name,
                        "type": if name == "priority" { "INTEGER" } else { "TEXT" },
                        "notnull": true,
                        "pk": name == "id"
                    })
                })
                .collect();

            Ok(json!({
                "table": "entities",
                "columns": cols
            }))
        } else {
            // Return distinct kinds as "tables"
            let rows = sqlx::query_scalar::<_, String>(
                "SELECT DISTINCT \"kind\" FROM \"entities\" ORDER BY \"kind\"",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|e| EmsError::db(format!("query error: {}", e)))?;

            Ok(json!({
                "tables": rows
            }))
        }
    }
}

// =============================================================================
// KIND MAPPING
// =============================================================================

/// Map logical table names to entity kind values.
/// Strips trailing 's' for plural table names (tasks→task, needs→need, wants→want).
/// Passes through singular names unchanged.
fn table_to_kind(table: &str) -> &str {
    match table {
        "tasks" => "task",
        "needs" => "need",
        "wants" => "want",
        "memories" => "memory",
        other => other,
    }
}

// =============================================================================
// PACK / UNPACK
// =============================================================================

type ColumnPairs = Vec<(String, Value)>;

/// Separate a map into (fixed_columns, extra_columns).
fn partition_columns(obj: &serde_json::Map<String, Value>) -> (ColumnPairs, ColumnPairs) {
    let mut fixed = Vec::new();
    let mut extras = Vec::new();
    for (k, v) in obj {
        if is_fixed_column(k) {
            fixed.push((k.clone(), v.clone()));
        } else {
            extras.push((k.clone(), v.clone()));
        }
    }
    (fixed, extras)
}

/// Unpack a database row: read all fixed columns, then parse the `data` blob
/// and merge its keys into the top-level object.
fn unpack_row(row: &sqlx::sqlite::SqliteRow) -> Value {
    let col_names: Vec<String> = row.columns().iter().map(|c| c.name().to_string()).collect();
    let mut obj = serde_json::Map::new();
    let mut data_json = String::new();

    for (i, name) in col_names.iter().enumerate() {
        let val = decode_value(read_column_as_json(row, i));
        if name == "data" {
            // Store raw data for later unpacking
            if let Value::String(s) = &val {
                data_json = s.clone();
            } else if let Value::Object(_) = &val {
                data_json = val.to_string();
            }
            // Don't insert "data" key into output
        } else {
            obj.insert(name.clone(), val);
        }
    }

    // Unpack data blob and merge into top-level
    if !data_json.is_empty()
        && let Ok(Value::Object(data_map)) = serde_json::from_str::<Value>(&data_json)
    {
        for (k, v) in data_map {
            // Don't overwrite fixed columns with data blob values
            if !obj.contains_key(&k) {
                obj.insert(k, v);
            }
        }
    }

    Value::Object(obj)
}

// =============================================================================
// IDENTIFIERS AND VALUE ENCODING
// =============================================================================

fn validate_identifier(name: &str) -> Result<(), EmsError> {
    let re = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap();
    if !re.is_match(name) || name.to_lowercase().starts_with("sqlite_") {
        return Err(EmsError::invalid_identifier(name));
    }
    Ok(())
}

fn encode_value(v: &Value) -> Value {
    match v {
        Value::Object(_) | Value::Array(_) => Value::String(v.to_string()),
        _ => v.clone(),
    }
}

fn decode_value(v: Value) -> Value {
    if let Value::String(s) = &v {
        let trimmed = s.trim();
        if ((trimmed.starts_with('{') && trimmed.ends_with('}'))
            || (trimmed.starts_with('[') && trimmed.ends_with(']')))
            && let Ok(parsed) = serde_json::from_str(trimmed)
        {
            return parsed;
        }
    }
    v
}

fn bind_json_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    val: &'q Value,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    match val {
        Value::Null => query.bind(None::<String>),
        Value::Bool(b) => query.bind(if *b { 1i64 } else { 0i64 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                query.bind(i)
            } else {
                query.bind(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => query.bind(s.as_str()),
        Value::Array(_) | Value::Object(_) => query.bind(val.to_string()),
    }
}

fn bind_json_value_scalar<'q, T>(
    query: sqlx::query::QueryScalar<'q, sqlx::Sqlite, T, sqlx::sqlite::SqliteArguments<'q>>,
    val: &'q Value,
) -> sqlx::query::QueryScalar<'q, sqlx::Sqlite, T, sqlx::sqlite::SqliteArguments<'q>> {
    match val {
        Value::Null => query.bind(None::<String>),
        Value::Bool(b) => query.bind(if *b { 1i64 } else { 0i64 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                query.bind(i)
            } else {
                query.bind(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => query.bind(s.as_str()),
        Value::Array(_) | Value::Object(_) => query.bind(val.to_string()),
    }
}

fn read_column_as_json(row: &sqlx::sqlite::SqliteRow, idx: usize) -> Value {
    if let Ok(Some(s)) = row.try_get::<Option<String>, _>(idx) {
        return Value::String(s);
    }
    if let Ok(Some(i)) = row.try_get::<Option<i64>, _>(idx) {
        return json!(i);
    }
    if let Ok(Some(f)) = row.try_get::<Option<f64>, _>(idx) {
        return json!(f);
    }
    Value::Null
}

// =============================================================================
// QUERY HELPERS
// =============================================================================

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
                if d == "DESC" { "DESC" } else { "ASC" }
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
                        if d == "DESC" { "DESC" } else { "ASC" }
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

    #[test]
    fn test_table_to_kind() {
        assert_eq!(table_to_kind("tasks"), "task");
        assert_eq!(table_to_kind("needs"), "need");
        assert_eq!(table_to_kind("wants"), "want");
        assert_eq!(table_to_kind("memories"), "memory");
        assert_eq!(table_to_kind("custom"), "custom");
    }

    #[test]
    fn test_partition_columns() {
        let obj: serde_json::Map<String, Value> = serde_json::from_value(json!({
            "id": "abc",
            "status": "pending",
            "prompt": "do something",
            "head_id": "h1",
            "batch_calls": "[]"
        }))
        .unwrap();

        let (fixed, extras) = partition_columns(&obj);
        let fixed_keys: Vec<&str> = fixed.iter().map(|(k, _)| k.as_str()).collect();
        let extra_keys: Vec<&str> = extras.iter().map(|(k, _)| k.as_str()).collect();

        assert!(fixed_keys.contains(&"id"));
        assert!(fixed_keys.contains(&"status"));
        assert!(fixed_keys.contains(&"prompt"));
        assert!(extra_keys.contains(&"head_id"));
        assert!(extra_keys.contains(&"batch_calls"));
    }
}
