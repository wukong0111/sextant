use std::fmt;

/// Supported database drivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Driver {
    Postgres,
    Mysql,
    Sqlite,
}

impl fmt::Display for Driver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Driver::Postgres => write!(f, "postgres"),
            Driver::Mysql => write!(f, "mysql"),
            Driver::Sqlite => write!(f, "sqlite"),
        }
    }
}

/// A saved database connection definition.
#[derive(Debug, Clone)]
pub struct Connection {
    pub name: String,
    pub driver: Driver,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub database: Option<String>,
    pub ssl_mode: Option<String>,
    pub path: Option<String>,
    pub keyring_key: String,
}

/// A single column descriptor in a query result.
#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub type_name: String,
}

/// A cell value in a result row.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    String(String),
    Bytes(Vec<u8>),
    // Time variants deferred to later phases
}

impl fmt::Display for CellValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CellValue::Null => write!(f, "NULL"),
            CellValue::Bool(v) => write!(f, "{v}"),
            CellValue::I64(v) => write!(f, "{v}"),
            CellValue::F64(v) => write!(f, "{v}"),
            CellValue::String(v) => write!(f, "{v}"),
            CellValue::Bytes(v) => write!(f, "<{} bytes>", v.len()),
        }
    }
}

/// A row of cell values.
pub type Row = Vec<CellValue>;

/// The result of executing a SQL statement.
#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    pub rows_affected: Option<u64>,
}

/// Errors that can occur in sextant.
#[derive(Debug, thiserror::Error)]
pub enum SextantError {
    #[error("database error: {0}")]
    Database(String),
    #[error("connection error: {0}")]
    Connection(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown error: {0}")]
    Unknown(String),
}

pub type Result<T> = std::result::Result<T, SextantError>;

/// Trait for executing queries against a database connection.
///
/// Implementors are expected to be `Send + Sync` so they can be held across
/// await points and shared between tasks.
pub trait QueryExecutor: Send + Sync {
    /// Execute a SQL string and return the resulting rows / metadata.
    fn execute(
        &self,
        sql: &str,
    ) -> impl std::future::Future<Output = Result<QueryResult>> + Send;
}
