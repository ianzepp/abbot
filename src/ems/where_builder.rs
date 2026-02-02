use serde_json::Value;

use super::service::EmsError;

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
            Value::Null => {
                conditions.push(format!("\"{}\" IS NULL", col));
            }
            Value::Object(ops) if is_operator_object(ops) => {
                for (op, operand) in ops {
                    let (cond, op_params) = build_operator_condition(col, op, operand)?;
                    conditions.push(cond);
                    params.extend(op_params);
                }
            }
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

fn is_operator_object(obj: &serde_json::Map<String, Value>) -> bool {
    obj.keys().all(|k| k.starts_with('$'))
}

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
