use sqlx::sqlite::SqlitePool;

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

pub async fn apply(pool: &SqlitePool) -> Result<(), EmsError> {
    // Run bootstrap unconditionally (idempotent).
    for stmt in MIGRATIONS[0].sql.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        sqlx::query(stmt)
            .execute(pool)
            .await
            .map_err(|e| EmsError::db(format!("failed to bootstrap migrations: {e}")))?;
    }

    let applied: std::collections::HashSet<i64> = {
        let rows: Vec<(i64,)> =
            sqlx::query_as("SELECT version FROM schema_migrations ORDER BY version")
                .fetch_all(pool)
                .await
                .map_err(|e| EmsError::db(format!("failed to query applied migrations: {e}")))?;
        rows.into_iter().map(|(v,)| v).collect()
    };

    for m in MIGRATIONS {
        if applied.contains(&m.version) {
            continue;
        }

        let mut tx = pool
            .begin()
            .await
            .map_err(|e| EmsError::db(format!("failed to start migration transaction: {e}")))?;

        for stmt in m.sql.split(';') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            sqlx::query(stmt).execute(&mut *tx).await.map_err(|e| {
                EmsError::db(format!("failed to apply migration {}: {e}", m.version))
            })?;
        }

        sqlx::query("INSERT INTO schema_migrations (version, name) VALUES (?1, ?2)")
            .bind(m.version)
            .bind(m.name)
            .execute(&mut *tx)
            .await
            .map_err(|e| EmsError::db(format!("failed to record migration {}: {e}", m.version)))?;

        tx.commit()
            .await
            .map_err(|e| EmsError::db(format!("failed to commit migration {}: {e}", m.version)))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use tempfile::TempDir;

    #[tokio::test]
    async fn applies_migrations_and_records_versions() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("ems.db");
        let opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();

        apply(&pool).await.unwrap();

        let versions: Vec<(i64,)> =
            sqlx::query_as("SELECT version FROM schema_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        let versions: Vec<i64> = versions.into_iter().map(|(v,)| v).collect();

        assert!(versions.contains(&0));
        assert!(versions.contains(&1));
    }
}
