//! Domain types and traits for sextant.

use thiserror::Error;

/// Supported database drivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    Postgres,
    Mysql,
    Sqlite,
}

/// A saved database connection configuration.
///
/// Fields are optional at the struct level; per-driver validation lives in
/// `sextant-config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub name: String,
    pub driver: Driver,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub database: Option<String>,
    pub ssl_mode: Option<String>,
    pub path: Option<String>,
    pub keyring_key: Option<String>,
}

/// A single cell value in a result row.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    String(String),
    Bytes(Vec<u8>),
}

/// Metadata for a result column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub type_name: String,
}

/// The result of executing a SQL statement.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<CellValue>>,
    pub rows_affected: Option<u64>,
}

/// Errors that can occur across the sextant stack.
#[derive(Debug, Error)]
pub enum SextantError {
    #[error("database error: {0}")]
    Database(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Trait for executing SQL queries against a database.
///
/// The `Send + Sync` supertraits ensure the executor can be shared across
/// async tasks (e.g. `tokio::spawn`).
pub trait QueryExecutor: Send + Sync {
    /// Execute a SQL statement and return the result.
    fn execute(
        &self,
        sql: &str,
    ) -> impl std::future::Future<Output = Result<QueryResult, SextantError>> + Send;
}


