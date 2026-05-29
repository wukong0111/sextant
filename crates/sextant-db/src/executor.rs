//! SQL query executor backed by sqlx.

use sextant_core::{CellValue, Column, QueryExecutor, QueryResult, SextantError};
use sqlx::{Column as _, Database, Row, TypeInfo};

/// A pooled database connection.
#[derive(Debug, Clone)]
pub enum DbPool {
    Postgres(sqlx::PgPool),
    MySql(sqlx::MySqlPool),
    Sqlite(sqlx::SqlitePool),
}

/// An async query executor using a sqlx connection pool.
#[derive(Debug, Clone)]
pub struct SqlxExecutor {
    pool: DbPool,
}

impl SqlxExecutor {
    /// Wrap an existing pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Access the underlying pool (crate-private).
    pub(crate) fn pool(&self) -> &DbPool {
        &self.pool
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

impl QueryExecutor for SqlxExecutor {
    async fn execute(&self, sql: &str) -> Result<QueryResult, SextantError> {
        let trimmed = sql.trim_start();
        let is_select = is_select_query(trimmed);

        match (&self.pool, is_select) {
            (DbPool::Postgres(pool), true) => {
                let rows = sqlx::query::<sqlx::Postgres>(sqlx::AssertSqlSafe(sql))
                    .fetch_all(pool)
                    .await
                    .map_err(|e| SextantError::Database(format!("query failed: {e}")))?;

                if rows.is_empty() {
                    return Ok(QueryResult {
                        columns: vec![],
                        rows: vec![],
                        rows_affected: None,
                    });
                }

                let columns = extract_columns(&rows[0]);
                let data = rows.iter().map(map_pg_row).collect::<Result<_, _>>()?;

                Ok(QueryResult {
                    columns,
                    rows: data,
                    rows_affected: None,
                })
            }
            (DbPool::Postgres(pool), false) => {
                let result = sqlx::query::<sqlx::Postgres>(sqlx::AssertSqlSafe(sql))
                    .execute(pool)
                    .await
                    .map_err(|e| SextantError::Database(format!("query failed: {e}")))?;

                Ok(QueryResult {
                    columns: vec![],
                    rows: vec![],
                    rows_affected: Some(result.rows_affected()),
                })
            }
            (DbPool::MySql(pool), true) => {
                let rows = sqlx::query::<sqlx::MySql>(sqlx::AssertSqlSafe(sql))
                    .fetch_all(pool)
                    .await
                    .map_err(|e| SextantError::Database(format!("query failed: {e}")))?;

                if rows.is_empty() {
                    return Ok(QueryResult {
                        columns: vec![],
                        rows: vec![],
                        rows_affected: None,
                    });
                }

                let columns = extract_columns(&rows[0]);
                let data = rows.iter().map(map_mysql_row).collect::<Result<_, _>>()?;

                Ok(QueryResult {
                    columns,
                    rows: data,
                    rows_affected: None,
                })
            }
            (DbPool::MySql(pool), false) => {
                let result = sqlx::query::<sqlx::MySql>(sqlx::AssertSqlSafe(sql))
                    .execute(pool)
                    .await
                    .map_err(|e| SextantError::Database(format!("query failed: {e}")))?;

                Ok(QueryResult {
                    columns: vec![],
                    rows: vec![],
                    rows_affected: Some(result.rows_affected()),
                })
            }
            (DbPool::Sqlite(pool), true) => {
                let rows = sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(sql))
                    .fetch_all(pool)
                    .await
                    .map_err(|e| SextantError::Database(format!("query failed: {e}")))?;

                if rows.is_empty() {
                    return Ok(QueryResult {
                        columns: vec![],
                        rows: vec![],
                        rows_affected: None,
                    });
                }

                let columns = extract_columns(&rows[0]);
                let data = rows.iter().map(map_sqlite_row).collect::<Result<_, _>>()?;

                Ok(QueryResult {
                    columns,
                    rows: data,
                    rows_affected: None,
                })
            }
            (DbPool::Sqlite(pool), false) => {
                let result = sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(sql))
                    .execute(pool)
                    .await
                    .map_err(|e| SextantError::Database(format!("query failed: {e}")))?;

                Ok(QueryResult {
                    columns: vec![],
                    rows: vec![],
                    rows_affected: Some(result.rows_affected()),
                })
            }
        }
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
}
