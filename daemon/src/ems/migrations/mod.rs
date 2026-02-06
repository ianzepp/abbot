use rusqlite::{params, Connection};

use crate::ems::service::EmsError;

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 0,
        name: "bootstrap",
        sql: include_str!("0000_bootstrap.sql"),
    },
    Migration {
        version: 1,
        name: "init",
        sql: include_str!("0001_init.sql"),
    },
];

pub fn apply(conn: &mut Connection) -> Result<(), EmsError> {
    // Run bootstrap unconditionally (idempotent).
    conn.execute_batch(MIGRATIONS[0].sql)
        .map_err(|e| EmsError::db(format!("failed to bootstrap migrations: {e}")))?;

    let applied: std::collections::HashSet<i64> = {
        let mut stmt = conn
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .map_err(|e| EmsError::db(format!("failed to prepare migrations query: {e}")))?;

        let rows = stmt
            .query_map([], |row| row.get::<_, i64>(0))
            .map_err(|e| EmsError::db(format!("failed to query applied migrations: {e}")))?;

        let mut out = std::collections::HashSet::new();
        for r in rows {
            out.insert(r.map_err(|e| EmsError::db(format!("failed to read migration row: {e}")))?);
        }
        out
    };

    for m in MIGRATIONS {
        if applied.contains(&m.version) {
            continue;
        }

        let tx = conn
            .transaction()
            .map_err(|e| EmsError::db(format!("failed to start migration transaction: {e}")))?;

        tx.execute_batch(m.sql)
            .map_err(|e| EmsError::db(format!("failed to apply migration {}: {e}", m.version)))?;

        tx.execute(
            "INSERT INTO schema_migrations (version, name) VALUES (?1, ?2)",
            params![m.version, m.name],
        )
        .map_err(|e| EmsError::db(format!("failed to record migration {}: {e}", m.version)))?;

        tx.commit()
            .map_err(|e| EmsError::db(format!("failed to commit migration {}: {e}", m.version)))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn applies_migrations_and_records_versions() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("ems.db");
        let mut conn = Connection::open(db_path).unwrap();

        apply(&mut conn).unwrap();

        let versions: Vec<i64> = conn
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(versions.contains(&0));
        assert!(versions.contains(&1));
    }
}
