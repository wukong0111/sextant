//! SQL query executor backed by sqlx.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sextant_core::{CellValue, Column, QueryExecutor, QueryResult, SextantError};
use sqlx::pool::PoolConnection;
use sqlx::{Column as _, Database, Row, TypeInfo};
use tokio::sync::Mutex;

/// A pooled database connection.
#[derive(Debug, Clone)]
pub enum DbPool {
    Postgres(sqlx::PgPool),
    MySql(sqlx::MySqlPool),
    Sqlite(sqlx::SqlitePool),
}

/// A connection checked out of the pool to hold an interactive transaction
/// (`BEGIN` … `COMMIT`/`ROLLBACK`) open across multiple `execute` calls.
enum HeldConn {
    Postgres(PoolConnection<sqlx::Postgres>),
    MySql(PoolConnection<sqlx::MySql>),
    Sqlite(PoolConnection<sqlx::Sqlite>),
}

/// Session-transaction state shared across [`SqlxExecutor`] clones.
///
/// `conn` holds the checked-out connection while a transaction is open; `active`
/// mirrors `conn.is_some()` as a cheap, lock-free flag the UI can poll while
/// rendering the status line.
#[derive(Default)]
struct TxnState {
    conn: Mutex<Option<HeldConn>>,
    active: AtomicBool,
}

/// A statement's transaction-control intent, recognized by its leading keyword.
enum TxnControl {
    Begin,
    Commit,
    Rollback,
}

/// An async query executor using a sqlx connection pool.
#[derive(Clone)]
pub struct SqlxExecutor {
    pool: DbPool,
    /// Shared session-transaction state (see [`TxnState`]).
    txn: Arc<TxnState>,
}

impl std::fmt::Debug for SqlxExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlxExecutor")
            .field("pool", &self.pool)
            .field("in_transaction", &self.in_transaction())
            .finish()
    }
}

impl SqlxExecutor {
    /// Wrap an existing pool.
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            txn: Arc::new(TxnState::default()),
        }
    }

    /// Access the underlying pool (crate-private).
    pub(crate) fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Whether an interactive session transaction is currently open.
    ///
    /// Lock-free; safe to call from the render path.
    pub fn in_transaction(&self) -> bool {
        self.txn.active.load(Ordering::Relaxed)
    }

    /// Open a session transaction by running `sql` (a `BEGIN`/`START
    /// TRANSACTION`) on a freshly checked-out connection, which is then held.
    async fn begin_held(&self, sql: &str) -> Result<HeldConn, SextantError> {
        macro_rules! begin {
            ($db:ty, $pool:expr, $variant:path) => {{
                let mut conn = $pool
                    .acquire()
                    .await
                    .map_err(|e| SextantError::Database(format!("begin failed: {e}")))?;
                sqlx::query::<$db>(sqlx::AssertSqlSafe(sql))
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| SextantError::Database(format!("begin failed: {e}")))?;
                $variant(conn)
            }};
        }
        Ok(match &self.pool {
            DbPool::Postgres(p) => begin!(sqlx::Postgres, p, HeldConn::Postgres),
            DbPool::MySql(p) => begin!(sqlx::MySql, p, HeldConn::MySql),
            DbPool::Sqlite(p) => begin!(sqlx::Sqlite, p, HeldConn::Sqlite),
        })
    }

    /// Execute a batch of statements inside a single transaction.
    ///
    /// All statements run in order; on any error the transaction is rolled
    /// back (by dropping the uncommitted `Transaction`) and the error is
    /// returned. On success the transaction commits and the total number of
    /// affected rows is returned. Used by grid editing (Fase 2.2) to apply
    /// pending UPDATE/INSERT/DELETE atomically.
    pub async fn execute_transaction(&self, statements: &[String]) -> Result<u64, SextantError> {
        macro_rules! run_tx {
            ($db:ty, $pool:expr) => {{
                let mut tx = $pool
                    .begin()
                    .await
                    .map_err(|e| SextantError::Database(format!("begin failed: {e}")))?;
                let mut affected = 0u64;
                for sql in statements {
                    let result = sqlx::query::<$db>(sqlx::AssertSqlSafe(sql.as_str()))
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| SextantError::Database(format!("statement failed: {e}")))?;
                    affected += result.rows_affected();
                }
                tx.commit()
                    .await
                    .map_err(|e| SextantError::Database(format!("commit failed: {e}")))?;
                Ok(affected)
            }};
        }

        match &self.pool {
            DbPool::Postgres(pool) => run_tx!(sqlx::Postgres, pool),
            DbPool::MySql(pool) => run_tx!(sqlx::MySql, pool),
            DbPool::Sqlite(pool) => run_tx!(sqlx::Sqlite, pool),
        }
    }
}

/// Run a statement against an sqlx executor target (a pool or a held
/// connection), routing SELECT-like statements to `fetch_all` and everything
/// else to `execute`. `$mapper` maps a backend row to `Vec<CellValue>`.
macro_rules! run_query {
    ($db:ty, $target:expr, $sql:expr, $is_select:expr, $mapper:expr) => {{
        if $is_select {
            let rows = sqlx::query::<$db>(sqlx::AssertSqlSafe($sql))
                .fetch_all($target)
                .await
                .map_err(|e| SextantError::Database(format!("query failed: {e}")))?;
            if rows.is_empty() {
                QueryResult {
                    columns: vec![],
                    rows: vec![],
                    rows_affected: None,
                }
            } else {
                let columns = extract_columns(&rows[0]);
                let data = rows.iter().map($mapper).collect::<Result<_, _>>()?;
                QueryResult {
                    columns,
                    rows: data,
                    rows_affected: None,
                }
            }
        } else {
            let result = sqlx::query::<$db>(sqlx::AssertSqlSafe($sql))
                .execute($target)
                .await
                .map_err(|e| SextantError::Database(format!("query failed: {e}")))?;
            QueryResult {
                columns: vec![],
                rows: vec![],
                rows_affected: Some(result.rows_affected()),
            }
        }
    }};
}

impl QueryExecutor for SqlxExecutor {
    async fn execute(&self, sql: &str) -> Result<QueryResult, SextantError> {
        let trimmed = sql.trim_start();
        let is_select = is_select_query(trimmed);
        let control = txn_control(trimmed);

        let mut guard = self.txn.conn.lock().await;

        // Transaction control: open or close the held session connection.
        match control {
            Some(TxnControl::Begin) if guard.is_none() => {
                let held = self.begin_held(sql).await?;
                *guard = Some(held);
                self.txn.active.store(true, Ordering::Relaxed);
                return Ok(empty_result());
            }
            Some(TxnControl::Commit | TxnControl::Rollback) if guard.is_some() => {
                let held = guard.take().expect("guard checked is_some");
                self.txn.active.store(false, Ordering::Relaxed);
                close_held(held, sql).await?;
                return Ok(empty_result());
            }
            // A stray COMMIT/ROLLBACK outside a transaction, or a nested BEGIN,
            // falls through and is run normally so the backend reports it.
            _ => {}
        }

        // Run on the held connection if a transaction is open, else on the pool.
        let result = match guard.as_mut() {
            Some(HeldConn::Postgres(c)) => {
                run_query!(sqlx::Postgres, &mut **c, sql, is_select, map_pg_row)
            }
            Some(HeldConn::MySql(c)) => {
                run_query!(sqlx::MySql, &mut **c, sql, is_select, map_mysql_row)
            }
            Some(HeldConn::Sqlite(c)) => {
                run_query!(sqlx::Sqlite, &mut **c, sql, is_select, map_sqlite_row)
            }
            None => match &self.pool {
                DbPool::Postgres(p) => run_query!(sqlx::Postgres, p, sql, is_select, map_pg_row),
                DbPool::MySql(p) => run_query!(sqlx::MySql, p, sql, is_select, map_mysql_row),
                DbPool::Sqlite(p) => run_query!(sqlx::Sqlite, p, sql, is_select, map_sqlite_row),
            },
        };
        Ok(result)
    }
}

/// Run a `COMMIT`/`ROLLBACK` on the held connection, then drop it so it returns
/// to the pool.
async fn close_held(held: HeldConn, sql: &str) -> Result<(), SextantError> {
    macro_rules! close {
        ($db:ty, $conn:expr) => {{
            let mut conn = $conn;
            sqlx::query::<$db>(sqlx::AssertSqlSafe(sql))
                .execute(&mut *conn)
                .await
                .map_err(|e| SextantError::Database(format!("transaction end failed: {e}")))?;
        }};
    }
    match held {
        HeldConn::Postgres(c) => close!(sqlx::Postgres, c),
        HeldConn::MySql(c) => close!(sqlx::MySql, c),
        HeldConn::Sqlite(c) => close!(sqlx::Sqlite, c),
    }
    Ok(())
}

/// An empty result for statements that return no rows (e.g. transaction control).
fn empty_result() -> QueryResult {
    QueryResult {
        columns: vec![],
        rows: vec![],
        rows_affected: None,
    }
}

/// Classify a statement's leading keyword as transaction control, if any.
fn txn_control(sql: &str) -> Option<TxnControl> {
    let upper = sql.trim_start().to_ascii_uppercase();
    let first = upper
        .split(|c: char| c.is_whitespace() || c == ';')
        .next()
        .unwrap_or("");
    match first {
        // `START TRANSACTION` and `BEGIN [TRANSACTION]` both open a transaction.
        "BEGIN" | "START" => Some(TxnControl::Begin),
        // `END` is a SQL synonym for `COMMIT`.
        "COMMIT" | "END" => Some(TxnControl::Commit),
        "ROLLBACK" => Some(TxnControl::Rollback),
        _ => None,
    }
}

/// Heuristic to decide whether a statement returns rows.
fn is_select_query(sql: &str) -> bool {
    let upper = sql.trim_start().to_ascii_uppercase();
    upper.starts_with("SELECT")
        || upper.starts_with("WITH")
        || upper.starts_with("EXPLAIN")
        || upper.starts_with("VALUES")
}

fn extract_columns<R>(row: &R) -> Vec<Column>
where
    R: Row,
    <R::Database as Database>::TypeInfo: TypeInfo,
{
    row.columns()
        .iter()
        .map(|c| Column {
            name: c.name().to_string(),
            type_name: c.type_info().name().to_string(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// PostgreSQL row mapping
// ---------------------------------------------------------------------------

fn map_pg_row(row: &sqlx::postgres::PgRow) -> Result<Vec<CellValue>, SextantError> {
    let mut out = Vec::with_capacity(row.columns().len());
    for i in 0..row.columns().len() {
        let type_name = row.columns()[i].type_info().name();
        out.push(map_pg_cell(row, i, type_name)?);
    }
    Ok(out)
}

fn map_pg_cell(
    row: &sqlx::postgres::PgRow,
    idx: usize,
    type_name: &str,
) -> Result<CellValue, SextantError> {
    let is_bool_type = type_name.to_ascii_lowercase().contains("bool");

    if is_bool_type {
        if let Ok(v) = row.try_get::<Option<bool>, _>(idx) {
            return Ok(v.map_or(CellValue::Null, CellValue::Bool));
        }
    }
    if let Ok(v) = row.try_get::<Option<i32>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, |v| CellValue::I64(i64::from(v))));
    }
    if let Ok(v) = row.try_get::<Option<i64>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::I64));
    }
    if let Ok(v) = row.try_get::<Option<f64>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::F64));
    }
    if let Ok(v) = row.try_get::<Option<sqlx::types::BigDecimal>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, |v| CellValue::String(v.to_string())));
    }
    if let Ok(v) = row.try_get::<Option<sqlx::types::chrono::NaiveDateTime>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, |v| CellValue::String(v.to_string())));
    }
    if let Ok(v) = row.try_get::<Option<sqlx::types::JsonValue>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, |v| CellValue::String(v.to_string())));
    }
    if let Ok(v) = row.try_get::<Option<String>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::String));
    }
    if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::Bytes));
    }
    Ok(CellValue::Null)
}

// ---------------------------------------------------------------------------
// MySQL row mapping
// ---------------------------------------------------------------------------

fn map_mysql_row(row: &sqlx::mysql::MySqlRow) -> Result<Vec<CellValue>, SextantError> {
    let mut out = Vec::with_capacity(row.columns().len());
    for i in 0..row.columns().len() {
        let type_name = row.columns()[i].type_info().name();
        out.push(map_mysql_cell(row, i, type_name)?);
    }
    Ok(out)
}

fn map_mysql_cell(
    row: &sqlx::mysql::MySqlRow,
    idx: usize,
    type_name: &str,
) -> Result<CellValue, SextantError> {
    let is_bool_type = type_name.to_ascii_lowercase().contains("bool");

    if is_bool_type {
        if let Ok(v) = row.try_get::<Option<bool>, _>(idx) {
            return Ok(v.map_or(CellValue::Null, CellValue::Bool));
        }
    }
    if let Ok(v) = row.try_get::<Option<i32>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, |v| CellValue::I64(i64::from(v))));
    }
    if let Ok(v) = row.try_get::<Option<i64>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::I64));
    }
    if let Ok(v) = row.try_get::<Option<f64>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::F64));
    }
    if let Ok(v) = row.try_get::<Option<sqlx::types::Decimal>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, |v| CellValue::String(v.to_string())));
    }
    if let Ok(v) = row.try_get::<Option<sqlx::types::chrono::NaiveDateTime>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, |v| CellValue::String(v.to_string())));
    }
    if let Ok(v) = row.try_get::<Option<sqlx::types::JsonValue>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, |v| CellValue::String(v.to_string())));
    }
    if let Ok(v) = row.try_get::<Option<String>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::String));
    }
    if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::Bytes));
    }
    Ok(CellValue::Null)
}

// ---------------------------------------------------------------------------
// SQLite row mapping
// ---------------------------------------------------------------------------

fn map_sqlite_row(row: &sqlx::sqlite::SqliteRow) -> Result<Vec<CellValue>, SextantError> {
    let mut out = Vec::with_capacity(row.columns().len());
    for i in 0..row.columns().len() {
        let type_name = row.columns()[i].type_info().name();
        out.push(map_sqlite_cell(row, i, type_name)?);
    }
    Ok(out)
}

fn map_sqlite_cell(
    row: &sqlx::sqlite::SqliteRow,
    idx: usize,
    type_name: &str,
) -> Result<CellValue, SextantError> {
    let is_bool_type = type_name.to_ascii_lowercase().contains("bool");

    if is_bool_type {
        if let Ok(v) = row.try_get::<Option<bool>, _>(idx) {
            return Ok(v.map_or(CellValue::Null, CellValue::Bool));
        }
    }
    if let Ok(v) = row.try_get::<Option<i32>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, |v| CellValue::I64(i64::from(v))));
    }
    if let Ok(v) = row.try_get::<Option<i64>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::I64));
    }
    if let Ok(v) = row.try_get::<Option<f64>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::F64));
    }
    if let Ok(v) = row.try_get::<Option<String>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::String));
    }
    if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::Bytes));
    }
    Ok(CellValue::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_select() {
        assert!(is_select_query("SELECT * FROM users"));
        assert!(is_select_query("  select id from t"));
        assert!(is_select_query("WITH cte AS (SELECT 1) SELECT * FROM cte"));
        assert!(is_select_query("EXPLAIN SELECT * FROM users"));
        assert!(is_select_query("VALUES (1, 2)"));
        assert!(!is_select_query("INSERT INTO t VALUES (1)"));
        assert!(!is_select_query("UPDATE t SET x = 1"));
        assert!(!is_select_query("DELETE FROM t"));
        assert!(!is_select_query("CREATE TABLE t (id INT)"));
    }

    #[test]
    fn classifies_txn_control() {
        assert!(matches!(txn_control("BEGIN"), Some(TxnControl::Begin)));
        assert!(matches!(
            txn_control("  begin transaction"),
            Some(TxnControl::Begin)
        ));
        assert!(matches!(
            txn_control("START TRANSACTION"),
            Some(TxnControl::Begin)
        ));
        assert!(matches!(txn_control("COMMIT"), Some(TxnControl::Commit)));
        assert!(matches!(txn_control("commit;"), Some(TxnControl::Commit)));
        assert!(matches!(txn_control("END"), Some(TxnControl::Commit)));
        assert!(matches!(
            txn_control("ROLLBACK"),
            Some(TxnControl::Rollback)
        ));
        assert!(txn_control("SELECT 1").is_none());
        assert!(txn_control("INSERT INTO t VALUES (1)").is_none());
        // A keyword that merely starts with "BEGIN" is not transaction control.
        assert!(txn_control("BEGINNING").is_none());
    }

    async fn sqlite_executor() -> SqlxExecutor {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        SqlxExecutor::new(DbPool::Sqlite(pool))
    }

    #[tokio::test]
    async fn session_transaction_rolls_back() {
        let exec = sqlite_executor().await;
        exec.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT)")
            .await
            .unwrap();
        assert!(!exec.in_transaction());

        exec.execute("BEGIN").await.unwrap();
        assert!(exec.in_transaction());
        exec.execute("INSERT INTO t (id, val) VALUES (1, 'a')")
            .await
            .unwrap();
        // The held connection sees its own uncommitted write.
        let inside = exec.execute("SELECT id FROM t").await.unwrap();
        assert_eq!(inside.rows.len(), 1);

        exec.execute("ROLLBACK").await.unwrap();
        assert!(!exec.in_transaction());
        let after = exec.execute("SELECT id FROM t").await.unwrap();
        assert!(after.rows.is_empty());
    }

    #[tokio::test]
    async fn session_transaction_commits() {
        let exec = sqlite_executor().await;
        exec.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT)")
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
    }
}
