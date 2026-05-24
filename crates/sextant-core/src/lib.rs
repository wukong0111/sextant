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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_round_trip() {
        assert_eq!(Driver::Postgres, Driver::Postgres);
        assert_ne!(Driver::Postgres, Driver::Sqlite);
    }

    #[test]
    fn cell_value_equality() {
        assert_eq!(CellValue::I64(42), CellValue::I64(42));
        assert_eq!(CellValue::Null, CellValue::Null);
        assert_ne!(CellValue::I64(42), CellValue::F64(42.0));
    }

    #[test]
    fn query_result_default() {
        let qr = QueryResult {
            columns: vec![Column {
                name: "id".into(),
                type_name: "INT4".into(),
            }],
            rows: vec![vec![CellValue::I64(1)]],
            rows_affected: None,
        };
        assert_eq!(qr.rows.len(), 1);
        assert_eq!(qr.columns.len(), 1);
    }

    #[test]
    fn sextant_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "nope");
        let err: SextantError = io_err.into();
        assert!(matches!(err, SextantError::Io(_)));
    }
}
