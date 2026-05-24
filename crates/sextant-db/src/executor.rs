//! SQL query executor backed by sqlx.

use sextant_core::{CellValue, Column, QueryExecutor, QueryResult, SextantError};
use sqlx::{AnyPool, AssertSqlSafe, Column as _, Row, TypeInfo as _};

/// An async query executor using a sqlx connection pool.
#[derive(Debug, Clone)]
pub struct SqlxExecutor {
    pool: AnyPool,
}

impl SqlxExecutor {
    /// Wrap an existing sqlx pool.
    pub fn new(pool: AnyPool) -> Self {
        Self { pool }
    }

    /// Access the underlying pool.
    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// Create a new pool from a connection URL.
    pub async fn connect(url: &str) -> Result<Self, SextantError> {
        install_drivers();
        let pool = AnyPool::connect(url)
            .await
            .map_err(|e| SextantError::Database(format!("failed to connect: {e}")))?;
        Ok(Self::new(pool))
    }
}

impl QueryExecutor for SqlxExecutor {
    async fn execute(&self, sql: &str) -> Result<QueryResult, SextantError> {
        let sql = sql.to_string();
        let trimmed = sql.trim_start();
        let is_select = is_select_query(trimmed);

        if is_select {
            let rows = sqlx::query(AssertSqlSafe(sql.clone()))
                .fetch_all(&self.pool)
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
            let data: Vec<Vec<CellValue>> = rows
                .iter()
                .map(|r| map_row(r))
                .collect::<Result<_, _>>()?;

            Ok(QueryResult {
                columns,
                rows: data,
                rows_affected: None,
            })
        } else {
            let result = sqlx::query(AssertSqlSafe(sql.clone()))
                .execute(&self.pool)
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

/// Heuristic to decide whether a statement returns rows.
fn is_select_query(sql: &str) -> bool {
    let upper = sql.trim_start().to_ascii_uppercase();
    upper.starts_with("SELECT")
        || upper.starts_with("WITH")
        || upper.starts_with("EXPLAIN")
        || upper.starts_with("VALUES")
}

fn extract_columns(row: &sqlx::any::AnyRow) -> Vec<Column> {
    row.columns()
        .iter()
        .map(|c| Column {
            name: c.name().to_string(),
            type_name: c.type_info().name().to_string(),
        })
        .collect()
}

fn map_row(row: &sqlx::any::AnyRow) -> Result<Vec<CellValue>, SextantError> {
    let mut out = Vec::with_capacity(row.columns().len());
    for i in 0..row.columns().len() {
        out.push(map_cell(row, i)?);
    }
    Ok(out)
}

pub(crate) fn install_drivers() {
    sqlx::any::install_default_drivers();
}

fn map_cell(row: &sqlx::any::AnyRow, idx: usize) -> Result<CellValue, SextantError> {
    // Try types from most specific to most general.
    if let Ok(v) = row.try_get::<Option<bool>, _>(idx) {
        return Ok(v.map_or(CellValue::Null, CellValue::Bool));
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
