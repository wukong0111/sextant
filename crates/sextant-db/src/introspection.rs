//! Database introspection: schemas and tables.

use sextant_core::{Driver, SextantError};
use sqlx::{AssertSqlSafe, Row};

use crate::executor::SqlxExecutor;

/// A schema with its tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    pub name: String,
    pub tables: Vec<String>,
}

/// List schemas and their tables for the given executor.
///
/// PostgreSQL uses `information_schema`. SQLite uses `PRAGMA database_list`
/// and per-database `sqlite_master`.
pub async fn introspect_schemas_and_tables(
    exec: &SqlxExecutor,
    driver: Driver,
) -> Result<Vec<Schema>, SextantError> {
    match driver {
        Driver::Postgres => introspect_postgres(exec).await,
        Driver::Sqlite => introspect_sqlite(exec).await,
        Driver::Mysql => Err(SextantError::Config(
            "MySQL introspection is not supported in v0.1".to_string(),
        )),
    }
}

async fn introspect_postgres(exec: &SqlxExecutor) -> Result<Vec<Schema>, SextantError> {
    let schema_rows = sqlx::query(
        "SELECT schema_name FROM information_schema.schemata \
         WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast') \
         ORDER BY schema_name",
    )
    .fetch_all(exec.pool())
    .await
    .map_err(|e| SextantError::Database(format!("failed to list schemas: {e}")))?;

    let mut schemas = Vec::with_capacity(schema_rows.len());
    for row in schema_rows {
        let name: String = row.try_get("schema_name").map_err(|e| {
            SextantError::Database(format!("failed to read schema_name: {e}"))
        })?;

        let table_rows = sqlx::query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = $1 AND table_type = 'BASE TABLE' \
             ORDER BY table_name",
        )
        .bind(&name)
        .fetch_all(exec.pool())
        .await
        .map_err(|e| SextantError::Database(format!("failed to list tables: {e}")))?;

        let tables = table_rows
            .into_iter()
            .map(|r| {
                r.try_get::<String, _>("table_name")
                    .map_err(|e| SextantError::Database(format!("failed to read table_name: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;

        schemas.push(Schema { name, tables });
    }

    Ok(schemas)
}

async fn introspect_sqlite(exec: &SqlxExecutor) -> Result<Vec<Schema>, SextantError> {
    let db_rows = sqlx::query("PRAGMA database_list")
        .fetch_all(exec.pool())
        .await
        .map_err(|e| SextantError::Database(format!("failed to list databases: {e}")))?;

    let mut schemas = Vec::with_capacity(db_rows.len());
    for row in db_rows {
        let name: String = row.try_get("name").map_err(|e| {
            SextantError::Database(format!("failed to read database name: {e}"))
        })?;

        // Skip the temporary database if it has no file.
        let file: Option<String> = row.try_get("file").ok();
        if name == "temp" && file.as_deref() == Some("") {
            continue;
        }

        let sql = format!(
            "SELECT name FROM \"{name}\".sqlite_master WHERE type='table' ORDER BY name"
        );
        let table_rows = sqlx::query(AssertSqlSafe(sql))
            .fetch_all(exec.pool())
            .await
            .map_err(|e| SextantError::Database(format!("failed to list tables: {e}")))?;

        let tables = table_rows
            .into_iter()
            .map(|r| {
                r.try_get::<String, _>("name")
                    .map_err(|e| SextantError::Database(format!("failed to read table name: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;

        schemas.push(Schema { name, tables });
    }

    Ok(schemas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sextant_core::QueryExecutor;

    async fn sqlite_executor() -> SqlxExecutor {
        crate::executor::install_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        SqlxExecutor::new(pool)
    }

    #[tokio::test]
    async fn sqlite_introspection() {
        let exec = sqlite_executor().await;

        exec.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)")
            .await
            .unwrap();
        exec.execute("CREATE TABLE orders (id INTEGER PRIMARY KEY)")
            .await
            .unwrap();

        let schemas = introspect_schemas_and_tables(&exec, Driver::Sqlite)
            .await
            .unwrap();

        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "main");
        assert_eq!(schemas[0].tables, vec!["orders", "users"]);
    }

    #[tokio::test]
    async fn sqlite_empty_database() {
        let exec = sqlite_executor().await;

        let schemas = introspect_schemas_and_tables(&exec, Driver::Sqlite)
            .await
            .unwrap();

        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "main");
        assert!(schemas[0].tables.is_empty());
    }

    #[tokio::test]
    async fn postgres_not_available_at_runtime() {
        // This test just verifies the function path exists for Postgres.
        // Real PG testing requires a running server (see executor tests).
        let exec = sqlite_executor().await;
        let result = introspect_schemas_and_tables(&exec, Driver::Postgres).await;
        // Will fail because sqlite doesn't have information_schema.schemata,
        // but we verify the code path compiles and runs.
        assert!(result.is_err());
    }
}
