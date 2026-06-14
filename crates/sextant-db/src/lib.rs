//! Database drivers and query execution.

pub mod connection_manager;
pub mod executor;
pub mod export;
pub mod import;
pub mod introspection;
pub mod sql;
pub mod url_builder;

pub use connection_manager::ConnectionManager;
pub use executor::{DbPool, SqlxExecutor};
pub use export::ExportFormat;
pub use import::{ImportData, ImportPreview};
pub use sql::{
    build_delete, build_insert, build_update, dangerous_reason, generate_create_table,
    qualified_table, quote_ident, to_sql_literal,
};
pub use url_builder::build_connection_url;

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use sextant_core::{CellValue, Driver, QueryExecutor};
    use sqlx::Row;
    use tokio::sync::Mutex;

    use super::*;

    /// Integration tests against real PostgreSQL/MySQL databases share a single
    /// schema and may create/drop tables with the same names, so they must run
    /// serially. SQLite tests use in-memory databases and do not need the lock.
    static DB_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn db_lock() -> &'static Mutex<()> {
        DB_TEST_LOCK.get_or_init(|| Mutex::new(()))
    }

    async fn db_guard() -> tokio::sync::MutexGuard<'static, ()> {
        db_lock().lock().await
    }

    async fn sqlite_executor() -> SqlxExecutor {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        SqlxExecutor::new(DbPool::Sqlite(pool))
    }

    async fn pg_executor() -> Option<SqlxExecutor> {
        let url = std::env::var("SEXTANT_TEST_PG_URL").unwrap_or_default();
        if url.is_empty() {
            return None;
        }
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        Some(SqlxExecutor::new(DbPool::Postgres(pool)))
    }

    async fn mysql_executor() -> Option<SqlxExecutor> {
        let url = std::env::var("SEXTANT_TEST_MYSQL_URL").unwrap_or_default();
        if url.is_empty() {
            return None;
        }
        let pool = sqlx::MySqlPool::connect(&url).await.unwrap();
        Some(SqlxExecutor::new(DbPool::MySql(pool)))
    }

    #[tokio::test]
    async fn sqlite_create_and_select() {
        let exec = sqlite_executor().await;

        exec.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, active INTEGER, score REAL, data BLOB)")
            .await
            .unwrap();

        exec.execute("INSERT INTO users (id, name, active, score, data) VALUES (1, 'alice', 1, 95.5, X'DEADBEEF')")
            .await
            .unwrap();

        exec.execute(
            "INSERT INTO users (id, name, active, score, data) VALUES (2, NULL, 0, NULL, NULL)",
        )
        .await
        .unwrap();

        let result = exec
            .execute("SELECT id, name, active, score, data FROM users ORDER BY id")
            .await
            .unwrap();

        assert_eq!(result.columns.len(), 5);
        assert_eq!(result.rows.len(), 2);

        // Row 1: alice
        let row1 = &result.rows[0];
        assert_eq!(row1[0], CellValue::I64(1));
        assert_eq!(row1[1], CellValue::String("alice".to_string()));
        assert_eq!(row1[2], CellValue::I64(1));
        assert_eq!(row1[3], CellValue::F64(95.5));
        assert_eq!(row1[4], CellValue::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]));

        // Row 2: nulls
        let row2 = &result.rows[1];
        assert_eq!(row2[0], CellValue::I64(2));
        assert_eq!(row2[1], CellValue::Null);
        assert_eq!(row2[2], CellValue::I64(0));
        assert_eq!(row2[3], CellValue::Null);
        assert_eq!(row2[4], CellValue::Null);
    }

    #[tokio::test]
    async fn sqlite_bool_column() {
        let exec = sqlite_executor().await;

        exec.execute("CREATE TABLE flags (id INTEGER PRIMARY KEY, enabled BOOLEAN)")
            .await
            .unwrap();
        exec.execute("INSERT INTO flags (enabled) VALUES (1), (0), (NULL)")
            .await
            .unwrap();

        let result = exec
            .execute("SELECT enabled FROM flags ORDER BY id")
            .await
            .unwrap();

        assert_eq!(result.rows.len(), 3);
        assert_eq!(result.rows[0][0], CellValue::Bool(true));
        assert_eq!(result.rows[1][0], CellValue::Bool(false));
        assert_eq!(result.rows[2][0], CellValue::Null);
    }

    #[tokio::test]
    async fn sqlite_dml_rows_affected() {
        let exec = sqlite_executor().await;

        exec.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT)")
            .await
            .unwrap();

        let insert = exec
            .execute("INSERT INTO t (val) VALUES ('a'), ('b'), ('c')")
            .await
            .unwrap();
        assert_eq!(insert.rows_affected, Some(3));
        assert!(insert.rows.is_empty());

        let update = exec
            .execute("UPDATE t SET val = 'x' WHERE id > 1")
            .await
            .unwrap();
        assert_eq!(update.rows_affected, Some(2));

        let delete = exec.execute("DELETE FROM t WHERE id = 1").await.unwrap();
        assert_eq!(delete.rows_affected, Some(1));
    }

    #[tokio::test]
    async fn sqlite_transaction_commits_all() {
        let exec = sqlite_executor().await;
        exec.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT)")
            .await
            .unwrap();

        let affected = exec
            .execute_transaction(&[
                "INSERT INTO t (id, val) VALUES (1, 'a')".to_string(),
                "INSERT INTO t (id, val) VALUES (2, 'b')".to_string(),
                "UPDATE t SET val = 'z' WHERE id = 1".to_string(),
            ])
            .await
            .unwrap();
        assert_eq!(affected, 3);

        let result = exec
            .execute("SELECT id, val FROM t ORDER BY id")
            .await
            .unwrap();
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][1], CellValue::String("z".to_string()));
    }

    #[tokio::test]
    async fn sqlite_transaction_rolls_back_on_error() {
        let exec = sqlite_executor().await;
        exec.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT)")
            .await
            .unwrap();
        exec.execute("INSERT INTO t (id, val) VALUES (1, 'a')")
            .await
            .unwrap();

        // Second statement violates the PK; the whole batch must roll back.
        let err = exec
            .execute_transaction(&[
                "INSERT INTO t (id, val) VALUES (2, 'b')".to_string(),
                "INSERT INTO t (id, val) VALUES (1, 'dup')".to_string(),
            ])
            .await;
        assert!(err.is_err());

        let result = exec.execute("SELECT id FROM t ORDER BY id").await.unwrap();
        // Only the original row 1 remains; row 2 was rolled back.
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], CellValue::I64(1));
    }

    #[tokio::test]
    async fn sqlite_empty_select() {
        let exec = sqlite_executor().await;

        exec.execute("CREATE TABLE empty (id INTEGER)")
            .await
            .unwrap();

        let result = exec.execute("SELECT * FROM empty").await.unwrap();

        assert!(result.rows.is_empty());
        assert!(result.columns.is_empty());
    }

    async fn cleanup_pg_schema(exec: &SqlxExecutor) {
        let DbPool::Postgres(pool) = exec.pool() else {
            return;
        };
        let _ = sqlx::query::<sqlx::Postgres>(
            "DO $$ DECLARE r RECORD; BEGIN \
             FOR r IN (SELECT tablename FROM pg_tables WHERE schemaname = 'public') LOOP \
             EXECUTE 'DROP TABLE IF EXISTS public.\"' || r.tablename || '\" CASCADE'; END LOOP; \
             END $$;",
        )
        .execute(pool)
        .await;
    }

    async fn cleanup_mysql_schema(exec: &SqlxExecutor) {
        let DbPool::MySql(pool) = exec.pool() else {
            return;
        };
        let _ = sqlx::query::<sqlx::MySql>("SET FOREIGN_KEY_CHECKS = 0")
            .execute(pool)
            .await;

        let rows = sqlx::query::<sqlx::MySql>(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'sextant_test' AND table_type = 'BASE TABLE'",
        )
        .fetch_all(pool)
        .await
        .unwrap();

        for row in rows {
            let name: String = row.try_get(0usize).unwrap();
            let sql = format!("DROP TABLE IF EXISTS `{}`", name);
            let _ = sqlx::query(sqlx::AssertSqlSafe(sql)).execute(pool).await;
        }

        let _ = sqlx::query::<sqlx::MySql>("SET FOREIGN_KEY_CHECKS = 1")
            .execute(pool)
            .await;
    }

    #[tokio::test]
    async fn postgresql_integration() {
        let _guard = db_guard().await;
        let Some(exec) = pg_executor().await else {
            return;
        };
        cleanup_pg_schema(&exec).await;

        exec.execute("CREATE TABLE pg_test (id INT PRIMARY KEY, label TEXT, amount NUMERIC)")
            .await
            .unwrap();

        exec.execute("INSERT INTO pg_test VALUES (1, 'hello', 42.00)")
            .await
            .unwrap();

        let result = exec
            .execute("SELECT id, label, amount FROM pg_test")
            .await
            .unwrap();

        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], CellValue::I64(1));
        assert_eq!(result.rows[0][1], CellValue::String("hello".to_string()));

        cleanup_pg_schema(&exec).await;
    }

    #[tokio::test]
    async fn postgresql_dml_rows_affected() {
        let _guard = db_guard().await;
        let Some(exec) = pg_executor().await else {
            return;
        };
        cleanup_pg_schema(&exec).await;

        exec.execute("CREATE TABLE t (id SERIAL PRIMARY KEY, val TEXT)")
            .await
            .unwrap();

        let insert = exec
            .execute("INSERT INTO t (val) VALUES ('a'), ('b'), ('c')")
            .await
            .unwrap();
        assert_eq!(insert.rows_affected, Some(3));
        assert!(insert.rows.is_empty());

        let update = exec
            .execute("UPDATE t SET val = 'x' WHERE id > 1")
            .await
            .unwrap();
        assert_eq!(update.rows_affected, Some(2));

        let delete = exec.execute("DELETE FROM t WHERE id = 1").await.unwrap();
        assert_eq!(delete.rows_affected, Some(1));

        cleanup_pg_schema(&exec).await;
    }

    #[tokio::test]
    async fn postgresql_session_transaction_rolls_back() {
        let _guard = db_guard().await;
        let Some(exec) = pg_executor().await else {
            return;
        };
        cleanup_pg_schema(&exec).await;

        exec.execute("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)")
            .await
            .unwrap();
        assert!(!exec.in_transaction());

        exec.execute("BEGIN").await.unwrap();
        assert!(exec.in_transaction());
        exec.execute("INSERT INTO t (id, val) VALUES (1, 'a')")
            .await
            .unwrap();

        let inside = exec.execute("SELECT id FROM t").await.unwrap();
        assert_eq!(inside.rows.len(), 1);

        exec.execute("ROLLBACK").await.unwrap();
        assert!(!exec.in_transaction());
        let after = exec.execute("SELECT id FROM t").await.unwrap();
        assert!(after.rows.is_empty());

        cleanup_pg_schema(&exec).await;
    }

    #[tokio::test]
    async fn postgresql_session_transaction_commits() {
        let _guard = db_guard().await;
        let Some(exec) = pg_executor().await else {
            return;
        };
        cleanup_pg_schema(&exec).await;

        exec.execute("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)")
            .await
            .unwrap();

        exec.execute("BEGIN").await.unwrap();
        exec.execute("INSERT INTO t (id, val) VALUES (1, 'a')")
            .await
            .unwrap();
        exec.execute("COMMIT").await.unwrap();
        assert!(!exec.in_transaction());

        let after = exec.execute("SELECT id FROM t").await.unwrap();
        assert_eq!(after.rows.len(), 1);

        cleanup_pg_schema(&exec).await;
    }

    #[tokio::test]
    async fn postgresql_transaction_commits_all() {
        let _guard = db_guard().await;
        let Some(exec) = pg_executor().await else {
            return;
        };
        cleanup_pg_schema(&exec).await;

        exec.execute("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)")
            .await
            .unwrap();

        let affected = exec
            .execute_transaction(&[
                "INSERT INTO t (id, val) VALUES (1, 'a')".to_string(),
                "INSERT INTO t (id, val) VALUES (2, 'b')".to_string(),
                "UPDATE t SET val = 'z' WHERE id = 1".to_string(),
            ])
            .await
            .unwrap();
        assert_eq!(affected, 3);

        let result = exec
            .execute("SELECT id, val FROM t ORDER BY id")
            .await
            .unwrap();
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][1], CellValue::String("z".to_string()));

        cleanup_pg_schema(&exec).await;
    }

    #[tokio::test]
    async fn postgresql_transaction_rolls_back_on_error() {
        let _guard = db_guard().await;
        let Some(exec) = pg_executor().await else {
            return;
        };
        cleanup_pg_schema(&exec).await;

        exec.execute("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)")
            .await
            .unwrap();
        exec.execute("INSERT INTO t (id, val) VALUES (1, 'a')")
            .await
            .unwrap();

        let err = exec
            .execute_transaction(&[
                "INSERT INTO t (id, val) VALUES (2, 'b')".to_string(),
                "INSERT INTO t (id, val) VALUES (1, 'dup')".to_string(),
            ])
            .await;
        assert!(err.is_err());

        let result = exec.execute("SELECT id FROM t ORDER BY id").await.unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], CellValue::I64(1));

        cleanup_pg_schema(&exec).await;
    }

    #[tokio::test]
    async fn postgresql_type_roundtrip() {
        let _guard = db_guard().await;
        let Some(exec) = pg_executor().await else {
            return;
        };
        cleanup_pg_schema(&exec).await;

        exec.execute(
            "CREATE TABLE type_test ( \
             id INT PRIMARY KEY, \
             b BOOLEAN, \
             i64 BIGINT, \
             f64 DOUBLE PRECISION, \
             num NUMERIC(10,2), \
             txt TEXT, \
             dt DATE, \
             ts TIMESTAMPTZ, \
             jb JSONB, \
             ba BYTEA, \
             ua UUID \
             )",
        )
        .await
        .unwrap();

        exec.execute(
            "INSERT INTO type_test VALUES ( \
            1, true, 9223372036854775807, 3.141592653589793, 1234.56, \
            'hello', '2024-06-15', '2024-06-15 14:30:00+00', '{\"a\":1}', \
            '\\xDEADBEEF', 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11' \
            )",
        )
        .await
        .unwrap();

        let result = exec
            .execute("SELECT b, i64, f64, num, txt, dt, ts, jb, ba, ua FROM type_test")
            .await
            .unwrap();

        assert_eq!(result.rows.len(), 1);
        let row = &result.rows[0];
        assert_eq!(row[0], CellValue::Bool(true));
        assert_eq!(row[1], CellValue::I64(9_223_372_036_854_775_807));
        assert_eq!(row[2], CellValue::F64(std::f64::consts::PI));
        let num = match &row[3] {
            CellValue::String(s) => s.clone(),
            other => panic!("expected numeric string, got {:?}", other),
        };
        assert!(
            num.starts_with("1234.56"),
            "unexpected numeric representation: {num}"
        );
        assert_eq!(row[4], CellValue::String("hello".to_string()));
        assert_eq!(row[5], CellValue::String("2024-06-15".to_string()));
        assert_eq!(
            row[6],
            CellValue::String("2024-06-15 14:30:00 +00:00".to_string())
        );
        let json = match &row[7] {
            CellValue::String(s) => s.clone(),
            other => panic!("expected JSON string, got {:?}", other),
        };
        assert!(json.contains("\"a\""));
        assert_eq!(row[8], CellValue::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]));
        assert_eq!(
            row[9],
            CellValue::String("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string())
        );

        cleanup_pg_schema(&exec).await;
    }

    #[tokio::test]
    async fn mysql_integration() {
        let _guard = db_guard().await;
        let Some(exec) = mysql_executor().await else {
            return;
        };
        cleanup_mysql_schema(&exec).await;

        exec.execute("CREATE TABLE mysql_test (id INT PRIMARY KEY, label VARCHAR(100), amount DECIMAL(10,2), created_at DATETIME, payload JSON)")
            .await
            .unwrap();

        exec.execute("INSERT INTO mysql_test VALUES (1, 'hello', 42.00, '2024-01-15 10:30:00', '{\"key\": \"value\"}')")
            .await
            .unwrap();

        let result = exec
            .execute("SELECT id, label, amount, created_at, payload FROM mysql_test")
            .await
            .unwrap();

        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], CellValue::I64(1));
        assert_eq!(result.rows[0][1], CellValue::String("hello".to_string()));
        assert_eq!(result.rows[0][2], CellValue::String("42.00".to_string()));
        assert_eq!(
            result.rows[0][3],
            CellValue::String("2024-01-15 10:30:00".to_string())
        );
        let json_str = match &result.rows[0][4] {
            CellValue::String(s) => s.clone(),
            other => panic!("expected String for JSON, got {:?}", other),
        };
        assert!(json_str.contains("\"key\""));
        assert!(json_str.contains("\"value\""));

        cleanup_mysql_schema(&exec).await;
    }

    #[tokio::test]
    async fn mysql_dml_rows_affected() {
        let _guard = db_guard().await;
        let Some(exec) = mysql_executor().await else {
            return;
        };
        cleanup_mysql_schema(&exec).await;

        exec.execute("CREATE TABLE t (id INT AUTO_INCREMENT PRIMARY KEY, val TEXT)")
            .await
            .unwrap();

        let insert = exec
            .execute("INSERT INTO t (val) VALUES ('a'), ('b'), ('c')")
            .await
            .unwrap();
        assert_eq!(insert.rows_affected, Some(3));
        assert!(insert.rows.is_empty());

        let update = exec
            .execute("UPDATE t SET val = 'x' WHERE id > 1")
            .await
            .unwrap();
        assert_eq!(update.rows_affected, Some(2));

        let delete = exec.execute("DELETE FROM t WHERE id = 1").await.unwrap();
        assert_eq!(delete.rows_affected, Some(1));

        cleanup_mysql_schema(&exec).await;
    }

    #[tokio::test]
    async fn mysql_session_transaction_rolls_back() {
        let _guard = db_guard().await;
        let Some(exec) = mysql_executor().await else {
            return;
        };
        cleanup_mysql_schema(&exec).await;

        exec.execute("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)")
            .await
            .unwrap();
        assert!(!exec.in_transaction());

        exec.execute("START TRANSACTION").await.unwrap();
        assert!(exec.in_transaction());
        exec.execute("INSERT INTO t (id, val) VALUES (1, 'a')")
            .await
            .unwrap();

        let inside = exec.execute("SELECT id FROM t").await.unwrap();
        assert_eq!(inside.rows.len(), 1);

        exec.execute("ROLLBACK").await.unwrap();
        assert!(!exec.in_transaction());
        let after = exec.execute("SELECT id FROM t").await.unwrap();
        assert!(after.rows.is_empty());

        cleanup_mysql_schema(&exec).await;
    }

    #[tokio::test]
    async fn mysql_session_transaction_commits() {
        let _guard = db_guard().await;
        let Some(exec) = mysql_executor().await else {
            return;
        };
        cleanup_mysql_schema(&exec).await;

        exec.execute("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)")
            .await
            .unwrap();

        exec.execute("START TRANSACTION").await.unwrap();
        exec.execute("INSERT INTO t (id, val) VALUES (1, 'a')")
            .await
            .unwrap();
        exec.execute("COMMIT").await.unwrap();
        assert!(!exec.in_transaction());

        let after = exec.execute("SELECT id FROM t").await.unwrap();
        assert_eq!(after.rows.len(), 1);

        cleanup_mysql_schema(&exec).await;
    }

    #[tokio::test]
    async fn mysql_transaction_commits_all() {
        let _guard = db_guard().await;
        let Some(exec) = mysql_executor().await else {
            return;
        };
        cleanup_mysql_schema(&exec).await;

        exec.execute("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)")
            .await
            .unwrap();

        let affected = exec
            .execute_transaction(&[
                "INSERT INTO t (id, val) VALUES (1, 'a')".to_string(),
                "INSERT INTO t (id, val) VALUES (2, 'b')".to_string(),
                "UPDATE t SET val = 'z' WHERE id = 1".to_string(),
            ])
            .await
            .unwrap();
        assert_eq!(affected, 3);

        let result = exec
            .execute("SELECT id, val FROM t ORDER BY id")
            .await
            .unwrap();
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][1], CellValue::String("z".to_string()));

        cleanup_mysql_schema(&exec).await;
    }

    #[tokio::test]
    async fn mysql_transaction_rolls_back_on_error() {
        let _guard = db_guard().await;
        let Some(exec) = mysql_executor().await else {
            return;
        };
        cleanup_mysql_schema(&exec).await;

        exec.execute("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)")
            .await
            .unwrap();
        exec.execute("INSERT INTO t (id, val) VALUES (1, 'a')")
            .await
            .unwrap();

        let err = exec
            .execute_transaction(&[
                "INSERT INTO t (id, val) VALUES (2, 'b')".to_string(),
                "INSERT INTO t (id, val) VALUES (1, 'dup')".to_string(),
            ])
            .await;
        assert!(err.is_err());

        let result = exec.execute("SELECT id FROM t ORDER BY id").await.unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], CellValue::I64(1));

        cleanup_mysql_schema(&exec).await;
    }

    #[tokio::test]
    async fn mysql_type_roundtrip() {
        let _guard = db_guard().await;
        let Some(exec) = mysql_executor().await else {
            return;
        };
        cleanup_mysql_schema(&exec).await;

        exec.execute(
            "CREATE TABLE type_test ( \
             id INT PRIMARY KEY, \
             b TINYINT(1), \
             i64 BIGINT, \
             f64 DOUBLE, \
             num DECIMAL(10,2), \
             txt TEXT, \
             dt DATE, \
             ts DATETIME, \
             jb JSON, \
             bl BLOB \
             )",
        )
        .await
        .unwrap();

        exec.execute(
            "INSERT INTO type_test VALUES ( \
            1, 1, 9223372036854775807, 3.141592653589793, 1234.56, \
            'hello', '2024-06-15', '2024-06-15 14:30:00', '{\"a\":1}', \
            X'DEADBEEF' \
            )",
        )
        .await
        .unwrap();

        let result = exec
            .execute("SELECT b, i64, f64, num, txt, dt, ts, jb, bl FROM type_test")
            .await
            .unwrap();

        assert_eq!(result.rows.len(), 1);
        let row = &result.rows[0];
        assert_eq!(row[0], CellValue::Bool(true));
        assert_eq!(row[1], CellValue::I64(9_223_372_036_854_775_807));
        assert_eq!(row[2], CellValue::F64(std::f64::consts::PI));
        let num = match &row[3] {
            CellValue::String(s) => s.clone(),
            other => panic!("expected numeric string, got {:?}", other),
        };
        assert!(
            num.starts_with("1234.56"),
            "unexpected numeric representation: {num}"
        );
        assert_eq!(row[4], CellValue::String("hello".to_string()));
        assert_eq!(row[5], CellValue::String("2024-06-15".to_string()));
        assert_eq!(row[6], CellValue::String("2024-06-15 14:30:00".to_string()));
        let json = match &row[7] {
            CellValue::String(s) => s.clone(),
            other => panic!("expected JSON string, got {:?}", other),
        };
        assert!(json.contains("\"a\""));
        assert_eq!(row[8], CellValue::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]));

        cleanup_mysql_schema(&exec).await;
    }

    #[tokio::test]
    async fn postgresql_introspection_integration() {
        let _guard = db_guard().await;
        let Some(exec) = pg_executor().await else {
            return;
        };
        cleanup_pg_schema(&exec).await;
        exec.execute("CREATE TABLE pg_introspect_test (id INT PRIMARY KEY)")
            .await
            .unwrap();

        let schemas = exec
            .introspect_schemas_and_tables(Driver::Postgres)
            .await
            .unwrap();
        let public = schemas
            .iter()
            .find(|s| s.name == "public")
            .expect("public schema should exist");
        assert!(public.tables.contains(&"pg_introspect_test".to_string()));

        cleanup_pg_schema(&exec).await;
    }

    #[tokio::test]
    async fn mysql_introspection_integration() {
        let _guard = db_guard().await;
        let Some(exec) = mysql_executor().await else {
            return;
        };
        cleanup_mysql_schema(&exec).await;
        exec.execute("CREATE TABLE mysql_introspect_test (id INT PRIMARY KEY)")
            .await
            .unwrap();

        let schemas = exec
            .introspect_schemas_and_tables(Driver::Mysql)
            .await
            .unwrap();
        let sextant_test = schemas
            .iter()
            .find(|s| s.name == "sextant_test")
            .expect("sextant_test schema should exist");
        assert!(
            sextant_test
                .tables
                .contains(&"mysql_introspect_test".to_string())
        );

        cleanup_mysql_schema(&exec).await;
    }

    #[tokio::test]
    async fn postgresql_columns_introspection_integration() {
        let _guard = db_guard().await;
        let Some(exec) = pg_executor().await else {
            return;
        };
        cleanup_pg_schema(&exec).await;
        exec.execute("CREATE TABLE cols_test (id INT PRIMARY KEY, name TEXT NOT NULL, note TEXT)")
            .await
            .unwrap();
        exec.execute("CREATE TABLE comp_test (a INT, b INT, val TEXT, PRIMARY KEY (b, a))")
            .await
            .unwrap();

        let tables = exec
            .introspect_columns(Driver::Postgres, "public")
            .await
            .unwrap();

        let (_, cols) = tables
            .iter()
            .find(|(t, _)| t == "cols_test")
            .expect("cols_test present");
        assert_eq!(cols.primary_key, vec!["id"]);
        assert!(
            cols.columns
                .iter()
                .find(|c| c.name == "id")
                .unwrap()
                .is_primary_key
        );
        assert!(
            !cols
                .columns
                .iter()
                .find(|c| c.name == "name")
                .unwrap()
                .nullable
        );
        assert!(
            cols.columns
                .iter()
                .find(|c| c.name == "note")
                .unwrap()
                .nullable
        );

        let (_, comp) = tables
            .iter()
            .find(|(t, _)| t == "comp_test")
            .expect("comp_test present");
        assert_eq!(comp.primary_key, vec!["b", "a"]);

        cleanup_pg_schema(&exec).await;
    }

    #[tokio::test]
    async fn mysql_columns_introspection_integration() {
        let _guard = db_guard().await;
        let Some(exec) = mysql_executor().await else {
            return;
        };
        cleanup_mysql_schema(&exec).await;
        exec.execute(
            "CREATE TABLE cols_test (id INT PRIMARY KEY, name VARCHAR(50) NOT NULL, note TEXT)",
        )
        .await
        .unwrap();
        exec.execute("CREATE TABLE comp_test (a INT, b INT, val TEXT, PRIMARY KEY (b, a))")
            .await
            .unwrap();

        let tables = exec
            .introspect_columns(Driver::Mysql, "sextant_test")
            .await
            .unwrap();

        let (_, cols) = tables
            .iter()
            .find(|(t, _)| t == "cols_test")
            .expect("cols_test present");
        assert_eq!(cols.primary_key, vec!["id"]);
        assert!(
            cols.columns
                .iter()
                .find(|c| c.name == "id")
                .unwrap()
                .is_primary_key
        );
        assert!(
            !cols
                .columns
                .iter()
                .find(|c| c.name == "name")
                .unwrap()
                .nullable
        );
        assert!(
            cols.columns
                .iter()
                .find(|c| c.name == "note")
                .unwrap()
                .nullable
        );

        let (_, comp) = tables
            .iter()
            .find(|(t, _)| t == "comp_test")
            .expect("comp_test present");
        let mut pk = comp.primary_key.clone();
        pk.sort();
        assert_eq!(pk, vec!["a", "b"]);

        cleanup_mysql_schema(&exec).await;
    }

    #[tokio::test]
    async fn postgresql_table_detail_integration() {
        let _guard = db_guard().await;
        let Some(exec) = pg_executor().await else {
            return;
        };
        cleanup_pg_schema(&exec).await;
        exec.execute("CREATE TABLE dept (id INT PRIMARY KEY)")
            .await
            .unwrap();
        exec.execute(
            "CREATE TABLE emp (id INT PRIMARY KEY, dept_id INT REFERENCES dept(id), name TEXT)",
        )
        .await
        .unwrap();
        exec.execute("CREATE UNIQUE INDEX idx_emp_name ON emp(name)")
            .await
            .unwrap();

        let detail = exec
            .introspect_table_detail(Driver::Postgres, "public", "emp")
            .await
            .unwrap();

        assert!(
            detail
                .indexes
                .iter()
                .any(|i| i.name == "idx_emp_name" && i.columns == vec!["name"] && i.unique)
        );
        let fk = detail
            .foreign_keys
            .iter()
            .find(|f| f.columns == vec!["dept_id"])
            .expect("fk present");
        assert_eq!(fk.ref_table, "dept");
        assert_eq!(fk.ref_columns, vec!["id"]);

        cleanup_pg_schema(&exec).await;
    }

    #[tokio::test]
    async fn mysql_table_detail_integration() {
        let _guard = db_guard().await;
        let Some(exec) = mysql_executor().await else {
            return;
        };
        cleanup_mysql_schema(&exec).await;
        exec.execute("CREATE TABLE dept (id INT PRIMARY KEY)")
            .await
            .unwrap();
        exec.execute(
            "CREATE TABLE emp (id INT PRIMARY KEY, dept_id INT, name VARCHAR(50), \
             FOREIGN KEY (dept_id) REFERENCES dept(id))",
        )
        .await
        .unwrap();
        exec.execute("CREATE UNIQUE INDEX idx_emp_name ON emp(name)")
            .await
            .unwrap();

        let detail = exec
            .introspect_table_detail(Driver::Mysql, "sextant_test", "emp")
            .await
            .unwrap();

        assert!(
            detail
                .indexes
                .iter()
                .any(|i| i.name == "idx_emp_name" && i.columns == vec!["name"] && i.unique)
        );
        let fk = detail
            .foreign_keys
            .iter()
            .find(|f| f.columns == vec!["dept_id"])
            .expect("fk present");
        assert_eq!(fk.ref_table, "dept");
        assert_eq!(fk.ref_columns, vec!["id"]);

        cleanup_mysql_schema(&exec).await;
    }
}
