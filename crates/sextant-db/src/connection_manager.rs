//! Manage active database connection pools.

use std::collections::HashMap;

use sextant_core::{Connection, SextantError};
use sqlx::AnyPool;

use crate::executor::SqlxExecutor;
use crate::url_builder::build_connection_url;

/// Maintains a map of named connection pools.
#[derive(Debug, Default)]
pub struct ConnectionManager {
    pools: HashMap<String, AnyPool>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            pools: HashMap::new(),
        }
    }

    /// Connect to a database and store the pool under `name`.
    ///
    /// `password` is used when building the connection URL.
    pub async fn connect(
        &mut self,
        name: &str,
        conn: &Connection,
        password: Option<&str>,
    ) -> Result<SqlxExecutor, SextantError> {
        crate::executor::install_drivers();
        let url = build_connection_url(conn, password)?;
        let pool = AnyPool::connect(&url)
            .await
            .map_err(|e| SextantError::Database(format!("failed to connect to {name}: {e}")))?;

        self.pools.insert(name.to_string(), pool.clone());
        Ok(SqlxExecutor::new(pool))
    }

    /// Disconnect and drop the pool associated with `name`.
    pub fn disconnect(&mut self, name: &str) {
        self.pools.remove(name);
    }

    /// Return an executor for an existing connection.
    pub fn get(&self, name: &str) -> Option<SqlxExecutor> {
        self.pools.get(name).cloned().map(SqlxExecutor::new)
    }

    /// Returns true if a pool with the given name exists.
    pub fn is_connected(&self, name: &str) -> bool {
        self.pools.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sextant_core::{Driver, QueryExecutor};

    fn sqlite_memory_conn() -> Connection {
        Connection {
            name: "test".to_string(),
            driver: Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl_mode: None,
            path: Some(":memory:".to_string()),
            keyring_key: None,
        }
    }

    #[tokio::test]
    async fn connect_and_disconnect_sqlite() {
        let mut mgr = ConnectionManager::new();
        let conn = sqlite_memory_conn();

        let exec = mgr.connect("mem", &conn, None).await.unwrap();
        assert!(mgr.is_connected("mem"));

        // Verify the executor works.
        let result = exec.execute("SELECT 1 as one").await.unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.columns.len(), 1);

        mgr.disconnect("mem");
        assert!(!mgr.is_connected("mem"));
        assert!(mgr.get("mem").is_none());
    }

    #[tokio::test]
    async fn multiple_independent_connections() {
        let mut mgr = ConnectionManager::new();
        let conn = sqlite_memory_conn();

        let exec_a = mgr.connect("a", &conn, None).await.unwrap();
        let exec_b = mgr.connect("b", &conn, None).await.unwrap();

        // Create a table in connection A.
        exec_a.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)")
            .await
            .unwrap();

        // Connection B should not see the table (separate in-memory DBs).
        let result = exec_b.execute("SELECT name FROM sqlite_master WHERE type='table'")
            .await
            .unwrap();
        assert!(result.rows.is_empty());
    }
}
