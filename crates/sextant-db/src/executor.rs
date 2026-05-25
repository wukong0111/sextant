//! SQL query executor backed by sqlx.

use sextant_core::{CellValue, Column, QueryExecutor, QueryResult, SextantError};
use sqlx::{Column as _, Database, Decode, Row, Type, TypeInfo};

/// A pooled database connection.
#[derive(Debug, Clone)]
pub enum DbPool {
    Postgres(sqlx::PgPool),
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
                let data = rows.iter().map(|r| map_row(r)).collect::<Result<_, _>>()?;

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
                let data = rows.iter().map(|r| map_row(r)).collect::<Result<_, _>>()?;

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

fn map_row<R>(row: &R) -> Result<Vec<CellValue>, SextantError>
where
    R: Row,
    <R::Database as Database>::TypeInfo: TypeInfo,
    usize: sqlx::ColumnIndex<R>,
    for<'a> bool: Decode<'a, R::Database> + Type<R::Database>,
    for<'a> i64: Decode<'a, R::Database> + Type<R::Database>,
    for<'a> f64: Decode<'a, R::Database> + Type<R::Database>,
    for<'a> String: Decode<'a, R::Database> + Type<R::Database>,
    for<'a> Vec<u8>: Decode<'a, R::Database> + Type<R::Database>,
{
    let mut out = Vec::with_capacity(row.columns().len());
    for i in 0..row.columns().len() {
        let type_name = row.columns()[i].type_info().name();
        out.push(map_cell(row, i, type_name)?);
    }
    Ok(out)
}

fn map_cell<R>(row: &R, idx: usize, type_name: &str) -> Result<CellValue, SextantError>
where
    R: Row,
    usize: sqlx::ColumnIndex<R>,
    for<'a> bool: Decode<'a, R::Database> + Type<R::Database>,
    for<'a> i64: Decode<'a, R::Database> + Type<R::Database>,
    for<'a> f64: Decode<'a, R::Database> + Type<R::Database>,
    for<'a> String: Decode<'a, R::Database> + Type<R::Database>,
    for<'a> Vec<u8>: Decode<'a, R::Database> + Type<R::Database>,
{
    let is_bool_type = type_name.to_ascii_lowercase().contains("bool");

    if is_bool_type {
        if let Ok(v) = row.try_get::<Option<bool>, _>(idx) {
            return Ok(v.map_or(CellValue::Null, CellValue::Bool));
        }
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
