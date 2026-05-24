//! Database drivers and query execution.

pub mod connection_manager;
pub mod executor;
pub mod introspection;
pub mod url_builder;

pub use connection_manager::ConnectionManager;
pub use executor::SqlxExecutor;
pub use url_builder::build_connection_url;

#[cfg(test)]
mod tests {
    use sextant_core::{CellValue, QueryExecutor};

    use super::*;

    async fn sqlite_executor() -> SqlxExecutor {
        executor::install_drivers();
        // Use a single-connection pool so that :memory: is shared across
        // queries from the same executor.
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        SqlxExecutor::new(pool)
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

        exec.execute("INSERT INTO users (id, name, active, score, data) VALUES (2, NULL, 0, NULL, NULL)")
            .await
            .unwrap();

        let result = exec.execute("SELECT id, name, active, score, data FROM users ORDER BY id")
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
    async fn sqlite_dml_rows_affected() {
        let exec = sqlite_executor().await;

        exec.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT)")
            .await
            .unwrap();

        let insert = exec.execute("INSERT INTO t (val) VALUES ('a'), ('b'), ('c')")
            .await
            .unwrap();
        assert_eq!(insert.rows_affected, Some(3));
        assert!(insert.rows.is_empty());

        let update = exec.execute("UPDATE t SET val = 'x' WHERE id > 1")
            .await
            .unwrap();
        assert_eq!(update.rows_affected, Some(2));

        let delete = exec.execute("DELETE FROM t WHERE id = 1")
            .await
            .unwrap();
        assert_eq!(delete.rows_affected, Some(1));
    }

    #[tokio::test]
    async fn sqlite_empty_select() {
        let exec = sqlite_executor().await;

        exec.execute("CREATE TABLE empty (id INTEGER)")
            .await
            .unwrap();

        let result = exec.execute("SELECT * FROM empty")
            .await
            .unwrap();

        assert!(result.rows.is_empty());
        assert!(result.columns.is_empty());
    }

    #[tokio::test]
    async fn postgresql_integration() {
        let url = std::env::var("SEXTANT_TEST_PG_URL").unwrap_or_default();
        if url.is_empty() {
            return; // skip if no test database is configured
        }

        let exec = SqlxExecutor::connect(&url).await.unwrap();

        exec.execute("CREATE TEMP TABLE pg_test (id INT PRIMARY KEY, label TEXT, amount NUMERIC)")
            .await
            .unwrap();

        exec.execute("INSERT INTO pg_test VALUES (1, 'hello', 42.00)")
            .await
            .unwrap();

        let result = exec.execute("SELECT id, label, amount FROM pg_test")
            .await
            .unwrap();

        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], CellValue::I64(1));
        assert_eq!(result.rows[0][1], CellValue::String("hello".to_string()));
    }
}
