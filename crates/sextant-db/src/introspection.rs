//! Database introspection: schemas and tables.

use sextant_core::{Driver, SextantError};
use sqlx::Row;

use crate::executor::{DbPool, SqlxExecutor};

/// A schema with its tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    pub name: String,
    pub tables: Vec<String>,
}

impl SqlxExecutor {
    /// List schemas and their tables for this executor.
    ///
    /// PostgreSQL uses `information_schema`. SQLite uses `PRAGMA database_list`
    /// and per-database `sqlite_master`.
    pub async fn introspect_schemas_and_tables(
        &self,
        driver: Driver,
    ) -> Result<Vec<Schema>, SextantError> {
        match driver {
            Driver::Postgres => self.introspect_postgres().await,
            Driver::Sqlite => self.introspect_sqlite().await,
            Driver::Mysql => self.introspect_mysql().await,
        }
    }

    async fn introspect_postgres(&self) -> Result<Vec<Schema>, SextantError> {
        let DbPool::Postgres(pool) = self.pool() else {
            return Err(SextantError::Database("expected postgres pool".to_string()));
        };

        let schema_rows = sqlx::query::<sqlx::Postgres>(
            "SELECT schema_name FROM information_schema.schemata \
             WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast') \
             ORDER BY schema_name",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| SextantError::Database(format!("failed to list schemas: {e}")))?;

        let mut schemas = Vec::with_capacity(schema_rows.len());
        for row in schema_rows {
            let name: String = row
                .try_get("schema_name")
                .map_err(|e| SextantError::Database(format!("failed to read schema_name: {e}")))?;

            let table_rows = sqlx::query::<sqlx::Postgres>(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = $1 AND table_type = 'BASE TABLE' \
                 ORDER BY table_name",
            )
            .bind(&name)
            .fetch_all(pool)
            .await
            .map_err(|e| SextantError::Database(format!("failed to list tables: {e}")))?;

            let tables = table_rows
                .into_iter()
                .map(|r| {
                    r.try_get::<String, _>("table_name").map_err(|e| {
                        SextantError::Database(format!("failed to read table_name: {e}"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;

            schemas.push(Schema { name, tables });
        }

        Ok(schemas)
    }

    async fn introspect_mysql(&self) -> Result<Vec<Schema>, SextantError> {
        let DbPool::MySql(pool) = self.pool() else {
            return Err(SextantError::Database("expected mysql pool".to_string()));
        };

        let schema_rows = sqlx::query::<sqlx::MySql>(
            "SELECT schema_name AS `schema_name` FROM information_schema.schemata \
             WHERE schema_name NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys') \
             ORDER BY schema_name",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| SextantError::Database(format!("failed to list schemas: {e}")))?;

        let mut schemas = Vec::with_capacity(schema_rows.len());
        for row in schema_rows {
            let name: String = row
                .try_get("schema_name")
                .map_err(|e| SextantError::Database(format!("failed to read schema_name: {e}")))?;

            let table_rows = sqlx::query::<sqlx::MySql>(
                "SELECT table_name AS `table_name` FROM information_schema.tables \
                 WHERE table_schema = ? AND table_type = 'BASE TABLE' \
                 ORDER BY table_name",
            )
            .bind(&name)
            .fetch_all(pool)
            .await
            .map_err(|e| SextantError::Database(format!("failed to list tables: {e}")))?;

            let tables = table_rows
                .into_iter()
                .map(|r| {
                    r.try_get::<String, _>("table_name").map_err(|e| {
                        SextantError::Database(format!("failed to read table_name: {e}"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;

            schemas.push(Schema { name, tables });
        }

        Ok(schemas)
    }

    async fn introspect_sqlite(&self) -> Result<Vec<Schema>, SextantError> {
        let DbPool::Sqlite(pool) = self.pool() else {
            return Err(SextantError::Database("expected sqlite pool".to_string()));
        };

        let db_rows = sqlx::query::<sqlx::Sqlite>("PRAGMA database_list")
            .fetch_all(pool)
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
            let table_rows = sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(sql))
                .fetch_all(pool)
                .await
                .map_err(|e| SextantError::Database(format!("failed to list tables: {e}")))?;

            let tables = table_rows
                .into_iter()
                .map(|r| {
                    r.try_get::<String, _>("name").map_err(|e| {
                        SextantError::Database(format!("failed to read table name: {e}"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;

            schemas.push(Schema { name, tables });
        }

        Ok(schemas)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sextant_core::QueryExecutor;

    async fn sqlite_executor() -> SqlxExecutor {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        SqlxExecutor::new(DbPool::Sqlite(pool))
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

        let schemas = exec
            .introspect_schemas_and_tables(Driver::Sqlite)
            .await
            .unwrap();

        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "main");
        assert_eq!(schemas[0].tables, vec!["orders", "users"]);
    }

    #[tokio::test]
    async fn sqlite_empty_database() {
        let exec = sqlite_executor().await;

        let schemas = exec
            .introspect_schemas_and_tables(Driver::Sqlite)
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
        let result = exec.introspect_schemas_and_tables(Driver::Postgres).await;
        // Will fail because sqlite doesn't have information_schema.schemata,
        // but we verify the code path compiles and runs.
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mysql_not_available_at_runtime() {
        // This test just verifies the function path exists for MySQL.
        let exec = sqlite_executor().await;
        let result = exec.introspect_schemas_and_tables(Driver::Mysql).await;
        // Will fail because sqlite doesn't have the expected MySQL pool,
        // but we verify the code path compiles and runs.
        assert!(result.is_err());
    }
}
