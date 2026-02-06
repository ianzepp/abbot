//! WHERE Clause Builder - MongoDB-Inspired Query Syntax for EMS
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module translates MongoDB-style query objects into SQL WHERE clauses
//! with bound parameters. It provides a familiar, JSON-friendly query syntax
//! that the LLM can construct without deep SQL knowledge.
//!
//! QUERY SYNTAX
//! ============
//! The where clause syntax supports:
//! - Equality: {"name": "value"} → "name" = ?
//! - Null checks: {"deleted_at": null} → "deleted_at" IS NULL
//! - Comparison: {"age": {"$gt": 18}} → "age" > ?
//! - Range queries: {"price": {"$gte": 10, "$lte": 100}} → "price" >= ? AND "price" <= ?
//! - Membership: {"status": {"$in": ["active", "pending"]}} → "status" IN (?, ?)
//! - Pattern matching: {"email": {"$like": "%@example.com"}} → "email" LIKE ?
//! - Inequality: {"deleted_at": {"$ne": null}} → "deleted_at" IS NOT NULL
//!
//! OPERATORS
//! =========
//! - $gt, $gte, $lt, $lte: Comparison operators (>, >=, <, <=)
//! - $ne: Not equal (!= or IS NOT NULL for null values)
//! - $in, $nin: Membership tests (IN, NOT IN)
//! - $like: Pattern matching (LIKE operator)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - JSON-native: Query syntax maps naturally to JSON objects, making it easy
//!   for the LLM to construct queries from structured data
//! - SQL injection safe: All values are bound as parameters, never interpolated
//! - Identifier validation: Column names are validated against a strict regex
//! - Progressive disclosure: Simple queries (equality) use simple syntax; complex
//!   queries (ranges, patterns) use operator syntax
//!
//! TRADE-OFFS
//! ==========
//! - Limited expressiveness: No support for OR, NOT, complex subqueries, or
//!   arbitrary SQL expressions. Keeps syntax simple but restricts some queries.
//! - AND-only logic: Multiple conditions are always combined with AND. OR would
//!   require a different syntax (e.g., {"$or": [...]}).
//! - No column-to-column comparisons: Can only compare columns to literal values,
//!   not to other columns (e.g., no {"start_date": {"$lt": "end_date"}}).
//!
//! SECURITY MODEL
//! ==============
//! - Identifier validation: Column names must match ^[A-Za-z_][A-Za-z0-9_]*$
//!   and cannot start with sqlite_, preventing SQL injection via identifiers
//! - Parameterized values: All values are bound as parameters, never concatenated
//!   into SQL strings
//! - Operator allowlist: Only recognized operators ($gt, $in, etc.) are accepted

use serde_json::Value;

use super::service::EmsError;

// =============================================================================
// MAIN BUILDER
// =============================================================================

/// Build a SQL WHERE clause from a MongoDB-style query object.
///
/// WHY MongoDB-style: Provides a familiar, structured query syntax that maps
/// naturally to JSON, making it easier for the LLM to construct queries.
///
/// Returns a tuple of (SQL string, bound parameters). If the where object is
/// empty or null, returns ("", []) to indicate no filtering.
///
/// SECURITY: Column names are validated against a strict regex before being
/// quoted and interpolated into the SQL string. Values are always bound as
/// parameters to prevent SQL injection.
pub fn build_where_clause(where_obj: &Value) -> Result<(String, Vec<Value>), EmsError> {
    let obj = match where_obj {
        Value::Object(m) if m.is_empty() => return Ok((String::new(), Vec::new())),
        Value::Object(m) => m,
        Value::Null => return Ok((String::new(), Vec::new())),
        _ => return Err(EmsError::db("where must be an object")),
    };

    let mut conditions = Vec::new();
    let mut params = Vec::new();

    for (col, value) in obj {
        validate_column_name(col)?;

        match value {
            // WHY special-case null: SQL uses IS NULL, not = NULL
            Value::Null => {
                conditions.push(format!("\"{}\" IS NULL", col));
            }
            // WHY operator object detection: Objects with all $-prefixed keys
            // are treated as operator expressions, not equality comparisons
            Value::Object(ops) if is_operator_object(ops) => {
                for (op, operand) in ops {
                    let (cond, op_params) = build_operator_condition(col, op, operand)?;
                    conditions.push(cond);
                    params.extend(op_params);
                }
            }
            // WHY default to equality: Simplest queries use the simplest syntax
            _ => {
                conditions.push(format!("\"{}\" = ?", col));
                params.push(value.clone());
            }
        }
    }

    if conditions.is_empty() {
        return Ok((String::new(), Vec::new()));
    }

    Ok((conditions.join(" AND "), params))
}

// =============================================================================
// VALIDATION
// =============================================================================

/// Validate a column name for use in SQL.
///
/// WHY: Column names cannot be parameterized in SQL, so they must be validated
/// before being quoted and interpolated into the query string. This prevents
/// SQL injection via malicious column names.
///
/// SECURITY: Rejects names that don't match ^[A-Za-z_][A-Za-z0-9_]*$ or that
/// start with sqlite_, which are reserved for SQLite internals.
fn validate_column_name(name: &str) -> Result<(), EmsError> {
    let re = regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap();
    if !re.is_match(name) || name.to_lowercase().starts_with("sqlite_") {
        return Err(EmsError::db(format!(
            "invalid column name in where: {}",
            name
        )));
    }
    Ok(())
}

// =============================================================================
// OPERATOR HANDLING
// =============================================================================

/// Check if a JSON object represents an operator expression.
///
/// WHY: Distinguishes {"$gt": 10} (operator) from {"nested": "value"} (equality).
/// Operator objects have all keys starting with $, while data objects don't.
fn is_operator_object(obj: &serde_json::Map<String, Value>) -> bool {
    obj.keys().all(|k| k.starts_with('$'))
}

/// Build a SQL condition from an operator and operand.
///
/// WHY separate function: Keeps operator logic isolated, making it easy to
/// add new operators or modify existing ones without affecting the main builder.
///
/// Returns a tuple of (SQL condition string, parameters to bind).
fn build_operator_condition(
    col: &str,
    op: &str,
    operand: &Value,
) -> Result<(String, Vec<Value>), EmsError> {
    match op {
        "$gt" => Ok((format!("\"{}\" > ?", col), vec![operand.clone()])),
        "$gte" => Ok((format!("\"{}\" >= ?", col), vec![operand.clone()])),
        "$lt" => Ok((format!("\"{}\" < ?", col), vec![operand.clone()])),
        "$lte" => Ok((format!("\"{}\" <= ?", col), vec![operand.clone()])),

        "$ne" => {
            // WHY special-case null: SQL uses IS NOT NULL, not != NULL
            if operand.is_null() {
                Ok((format!("\"{}\" IS NOT NULL", col), Vec::new()))
            } else {
                Ok((format!("\"{}\" != ?", col), vec![operand.clone()]))
            }
        }

        "$in" => {
            let arr = operand
                .as_array()
                .ok_or_else(|| EmsError::db("$in requires an array"))?;

            // WHY special-case empty array: IN () is invalid SQL; use 1=0 (always false)
            if arr.is_empty() {
                return Ok(("1=0".to_string(), Vec::new()));
            }

            let placeholders: Vec<&str> = arr.iter().map(|_| "?").collect();
            let condition = format!("\"{}\" IN ({})", col, placeholders.join(", "));
            Ok((condition, arr.clone()))
        }

        "$nin" => {
            let arr = operand
                .as_array()
                .ok_or_else(|| EmsError::db("$nin requires an array"))?;

            // WHY special-case empty array: NOT IN () is invalid SQL; use 1=1 (always true)
            if arr.is_empty() {
                return Ok(("1=1".to_string(), Vec::new()));
            }

            let placeholders: Vec<&str> = arr.iter().map(|_| "?").collect();
            let condition = format!("\"{}\" NOT IN ({})", col, placeholders.join(", "));
            Ok((condition, arr.clone()))
        }

        "$like" => {
            let pattern = operand
                .as_str()
                .ok_or_else(|| EmsError::db("$like requires a string"))?;
            Ok((
                format!("\"{}\" LIKE ?", col),
                vec![Value::String(pattern.to_string())],
            ))
        }

        _ => Err(EmsError::db(format!("unknown operator: {}", op))),
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_equality() {
        let (sql, params) = build_where_clause(&json!({"name": "test"})).unwrap();
        assert_eq!(sql, "\"name\" = ?");
        assert_eq!(params, vec![json!("test")]);
    }

    #[test]
    fn test_null_value() {
        let (sql, params) = build_where_clause(&json!({"deleted_at": null})).unwrap();
        assert_eq!(sql, "\"deleted_at\" IS NULL");
        assert!(params.is_empty());
    }

    #[test]
    fn test_gt_operator() {
        let (sql, params) = build_where_clause(&json!({"age": {"$gt": 18}})).unwrap();
        assert_eq!(sql, "\"age\" > ?");
        assert_eq!(params, vec![json!(18)]);
    }

    #[test]
    fn test_in_operator() {
        let (sql, params) =
            build_where_clause(&json!({"status": {"$in": ["active", "pending"]}})).unwrap();
        assert_eq!(sql, "\"status\" IN (?, ?)");
        assert_eq!(params, vec![json!("active"), json!("pending")]);
    }

    #[test]
    fn test_multiple_conditions() {
        let (sql, params) = build_where_clause(&json!({
            "status": "active",
            "age": {"$gte": 21}
        }))
        .unwrap();
        assert!(sql.contains("\"status\" = ?"));
        assert!(sql.contains("\"age\" >= ?"));
        assert!(sql.contains(" AND "));
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_empty_where() {
        let (sql, params) = build_where_clause(&json!({})).unwrap();
        assert!(sql.is_empty());
        assert!(params.is_empty());
    }

    #[test]
    fn test_empty_in_array() {
        let (sql, params) = build_where_clause(&json!({"id": {"$in": []}})).unwrap();
        assert_eq!(sql, "1=0");
        assert!(params.is_empty());
    }
}
