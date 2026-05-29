//! Database introspection: schemas and tables.

use std::collections::BTreeMap;

use sextant_core::{Driver, SextantError};
use sqlx::Row;

use crate::executor::{DbPool, SqlxExecutor};

/// A schema with its tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    pub name: String,
    pub tables: Vec<String>,
}

/// Metadata for a single column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMeta {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub is_primary_key: bool,
}

/// Column-level metadata for a table, including its primary key.
///
/// `primary_key` lists the PK column names in key order; it is empty when the
/// table has no primary key (which makes the table read-only for grid editing).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableMeta {
    pub columns: Vec<ColumnMeta>,
    pub primary_key: Vec<String>,
}

/// An index on a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexMeta {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

/// A foreign key from a table to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKeyMeta {
    pub name: String,
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

/// Per-table indexes and foreign keys, loaded lazily when a table is expanded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableDetail {
    pub indexes: Vec<IndexMeta>,
    pub foreign_keys: Vec<ForeignKeyMeta>,
}

impl SqlxExecutor {
    /// List schemas and their tables for this executor.
    ///
    /// PostgreSQL uses `information_schema`. SQLite uses `PRAGMA database_list`
    /// and per-database `sqlite_master`.
    pub async fn introspect_schemas_and_tables(
        &self,
        driver: Driver,
    ) -> Result<Vec<Schema>, SextantError> {
        match driver {
            Driver::Postgres => self.introspect_postgres().await,
            Driver::Sqlite => self.introspect_sqlite().await,
            Driver::Mysql => self.introspect_mysql().await,
        }
    }

    async fn introspect_postgres(&self) -> Result<Vec<Schema>, SextantError> {
        let DbPool::Postgres(pool) = self.pool() else {
            return Err(SextantError::Database("expected postgres pool".to_string()));
        };

        let schema_rows = sqlx::query::<sqlx::Postgres>(
            "SELECT schema_name FROM information_schema.schemata \
             WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast') \
             ORDER BY schema_name",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| SextantError::Database(format!("failed to list schemas: {e}")))?;

        let mut schemas = Vec::with_capacity(schema_rows.len());
        for row in schema_rows {
            let name: String = row
                .try_get("schema_name")
                .map_err(|e| SextantError::Database(format!("failed to read schema_name: {e}")))?;

            let table_rows = sqlx::query::<sqlx::Postgres>(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = $1 AND table_type = 'BASE TABLE' \
                 ORDER BY table_name",
            )
            .bind(&name)
            .fetch_all(pool)
            .await
            .map_err(|e| SextantError::Database(format!("failed to list tables: {e}")))?;

            let tables = table_rows
                .into_iter()
                .map(|r| {
                    r.try_get::<String, _>("table_name").map_err(|e| {
                        SextantError::Database(format!("failed to read table_name: {e}"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;

            schemas.push(Schema { name, tables });
        }

        Ok(schemas)
    }

    async fn introspect_mysql(&self) -> Result<Vec<Schema>, SextantError> {
        let DbPool::MySql(pool) = self.pool() else {
            return Err(SextantError::Database("expected mysql pool".to_string()));
        };

        let schema_rows = sqlx::query::<sqlx::MySql>(
            "SELECT schema_name AS `schema_name` FROM information_schema.schemata \
             WHERE schema_name NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys') \
             ORDER BY schema_name",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| SextantError::Database(format!("failed to list schemas: {e}")))?;

        let mut schemas = Vec::with_capacity(schema_rows.len());
        for row in schema_rows {
            let name: String = row
                .try_get("schema_name")
                .map_err(|e| SextantError::Database(format!("failed to read schema_name: {e}")))?;

            let table_rows = sqlx::query::<sqlx::MySql>(
                "SELECT table_name AS `table_name` FROM information_schema.tables \
                 WHERE table_schema = ? AND table_type = 'BASE TABLE' \
                 ORDER BY table_name",
            )
            .bind(&name)
            .fetch_all(pool)
            .await
            .map_err(|e| SextantError::Database(format!("failed to list tables: {e}")))?;

            let tables = table_rows
                .into_iter()
                .map(|r| {
                    r.try_get::<String, _>("table_name").map_err(|e| {
                        SextantError::Database(format!("failed to read table_name: {e}"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;

            schemas.push(Schema { name, tables });
        }

        Ok(schemas)
    }

    async fn introspect_sqlite(&self) -> Result<Vec<Schema>, SextantError> {
        let DbPool::Sqlite(pool) = self.pool() else {
            return Err(SextantError::Database("expected sqlite pool".to_string()));
        };

        let db_rows = sqlx::query::<sqlx::Sqlite>("PRAGMA database_list")
            .fetch_all(pool)
            .await
            .map_err(|e| SextantError::Database(format!("failed to list databases: {e}")))?;

        let mut schemas = Vec::with_capacity(db_rows.len());
        for row in db_rows {
            let name: String = row.try_get("name").map_err(|e| {
                SextantError::Database(format!("failed to read database name: {e}"))
            })?;

            // Skip the temporary database if it has no file.
            let file: Option<String> = row.try_get("file").ok();
            if name == "temp" && file.as_deref() == Some("") {
                continue;
            }

            let sql = format!(
                "SELECT name FROM \"{name}\".sqlite_master WHERE type='table' ORDER BY name"
            );
            let table_rows = sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(sql))
                .fetch_all(pool)
                .await
                .map_err(|e| SextantError::Database(format!("failed to list tables: {e}")))?;

            let tables = table_rows
                .into_iter()
                .map(|r| {
                    r.try_get::<String, _>("name").map_err(|e| {
                        SextantError::Database(format!("failed to read table name: {e}"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;

            schemas.push(Schema { name, tables });
        }

        Ok(schemas)
    }

    /// List column metadata (with primary keys) for every table in `schema`.
    ///
    /// Returns `(table_name, TableMeta)` pairs ordered by table name. Used to
    /// feed the schema viewer, autocomplete and grid-editing (PK detection).
    pub async fn introspect_columns(
        &self,
        driver: Driver,
        schema: &str,
    ) -> Result<Vec<(String, TableMeta)>, SextantError> {
        match driver {
            Driver::Postgres => self.columns_postgres(schema).await,
            Driver::Mysql => self.columns_mysql(schema).await,
            Driver::Sqlite => self.columns_sqlite(schema).await,
        }
    }

    async fn columns_postgres(
        &self,
        schema: &str,
    ) -> Result<Vec<(String, TableMeta)>, SextantError> {
        let DbPool::Postgres(pool) = self.pool() else {
            return Err(SextantError::Database("expected postgres pool".to_string()));
        };

        let col_rows = sqlx::query::<sqlx::Postgres>(
            "SELECT table_name, column_name, data_type, is_nullable, column_default \
             FROM information_schema.columns \
             WHERE table_schema = $1 \
             ORDER BY table_name, ordinal_position",
        )
        .bind(schema)
        .fetch_all(pool)
        .await
        .map_err(|e| SextantError::Database(format!("failed to list columns: {e}")))?;

        let pk_rows = sqlx::query::<sqlx::Postgres>(
            "SELECT kcu.table_name, kcu.column_name \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON tc.constraint_name = kcu.constraint_name \
              AND tc.table_schema = kcu.table_schema \
             WHERE tc.constraint_type = 'PRIMARY KEY' AND tc.table_schema = $1 \
             ORDER BY kcu.table_name, kcu.ordinal_position",
        )
        .bind(schema)
        .fetch_all(pool)
        .await
        .map_err(|e| SextantError::Database(format!("failed to list primary keys: {e}")))?;

        let mut pk_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for row in &pk_rows {
            let table: String = row
                .try_get("table_name")
                .map_err(|e| SextantError::Database(format!("failed to read pk table: {e}")))?;
            let column: String = row
                .try_get("column_name")
                .map_err(|e| SextantError::Database(format!("failed to read pk column: {e}")))?;
            pk_map.entry(table).or_default().push(column);
        }

        let mut cols_map: BTreeMap<String, Vec<ColumnMeta>> = BTreeMap::new();
        for row in &col_rows {
            let table: String = row
                .try_get("table_name")
                .map_err(|e| SextantError::Database(format!("failed to read table: {e}")))?;
            let is_nullable: String = row
                .try_get("is_nullable")
                .map_err(|e| SextantError::Database(format!("failed to read is_nullable: {e}")))?;
            cols_map.entry(table).or_default().push(ColumnMeta {
                name: row
                    .try_get("column_name")
                    .map_err(|e| SextantError::Database(format!("failed to read column: {e}")))?,
                type_name: row.try_get("data_type").map_err(|e| {
                    SextantError::Database(format!("failed to read data_type: {e}"))
                })?,
                nullable: is_nullable.eq_ignore_ascii_case("YES"),
                default: row
                    .try_get::<Option<String>, _>("column_default")
                    .ok()
                    .flatten(),
                is_primary_key: false,
            });
        }

        Ok(merge_columns(cols_map, pk_map))
    }

    async fn columns_mysql(&self, schema: &str) -> Result<Vec<(String, TableMeta)>, SextantError> {
        let DbPool::MySql(pool) = self.pool() else {
            return Err(SextantError::Database("expected mysql pool".to_string()));
        };

        let rows = sqlx::query::<sqlx::MySql>(
            "SELECT table_name AS `table_name`, column_name AS `column_name`, \
                    data_type AS `data_type`, is_nullable AS `is_nullable`, \
                    column_default AS `column_default`, column_key AS `column_key` \
             FROM information_schema.columns \
             WHERE table_schema = ? \
             ORDER BY table_name, ordinal_position",
        )
        .bind(schema)
        .fetch_all(pool)
        .await
        .map_err(|e| SextantError::Database(format!("failed to list columns: {e}")))?;

        let mut cols_map: BTreeMap<String, Vec<ColumnMeta>> = BTreeMap::new();
        let mut pk_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for row in &rows {
            let table: String = row
                .try_get("table_name")
                .map_err(|e| SextantError::Database(format!("failed to read table: {e}")))?;
            let name: String = row
                .try_get("column_name")
                .map_err(|e| SextantError::Database(format!("failed to read column: {e}")))?;
            let is_nullable: String = row
                .try_get("is_nullable")
                .map_err(|e| SextantError::Database(format!("failed to read is_nullable: {e}")))?;
            let column_key: String = row
                .try_get::<Option<String>, _>("column_key")
                .ok()
                .flatten()
                .unwrap_or_default();
            if column_key == "PRI" {
                pk_map.entry(table.clone()).or_default().push(name.clone());
            }
            cols_map.entry(table).or_default().push(ColumnMeta {
                name,
                type_name: row.try_get("data_type").map_err(|e| {
                    SextantError::Database(format!("failed to read data_type: {e}"))
                })?,
                nullable: is_nullable.eq_ignore_ascii_case("YES"),
                default: row
                    .try_get::<Option<String>, _>("column_default")
                    .ok()
                    .flatten(),
                is_primary_key: false,
            });
        }

        Ok(merge_columns(cols_map, pk_map))
    }

    async fn columns_sqlite(&self, schema: &str) -> Result<Vec<(String, TableMeta)>, SextantError> {
        let DbPool::Sqlite(pool) = self.pool() else {
            return Err(SextantError::Database("expected sqlite pool".to_string()));
        };

        let table_sql =
            format!("SELECT name FROM \"{schema}\".sqlite_master WHERE type='table' ORDER BY name");
        let table_rows = sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(table_sql))
            .fetch_all(pool)
            .await
            .map_err(|e| SextantError::Database(format!("failed to list tables: {e}")))?;

        let mut out = Vec::with_capacity(table_rows.len());
        for trow in &table_rows {
            let table: String = trow
                .try_get("name")
                .map_err(|e| SextantError::Database(format!("failed to read table name: {e}")))?;

            let pragma = format!("PRAGMA \"{schema}\".table_info(\"{table}\")");
            let col_rows = sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(pragma))
                .fetch_all(pool)
                .await
                .map_err(|e| SextantError::Database(format!("failed to read table_info: {e}")))?;

            let mut columns = Vec::with_capacity(col_rows.len());
            let mut pk: Vec<(i64, String)> = Vec::new();
            for row in &col_rows {
                let name: String = row
                    .try_get("name")
                    .map_err(|e| SextantError::Database(format!("failed to read column: {e}")))?;
                let notnull: i64 = row.try_get("notnull").unwrap_or(0);
                let pk_idx: i64 = row.try_get("pk").unwrap_or(0);
                if pk_idx > 0 {
                    pk.push((pk_idx, name.clone()));
                }
                columns.push(ColumnMeta {
                    name,
                    type_name: row
                        .try_get::<Option<String>, _>("type")
                        .ok()
                        .flatten()
                        .unwrap_or_default(),
                    nullable: notnull == 0,
                    default: row
                        .try_get::<Option<String>, _>("dflt_value")
                        .ok()
                        .flatten(),
                    is_primary_key: pk_idx > 0,
                });
            }
            pk.sort_by_key(|(idx, _)| *idx);
            let primary_key = pk.into_iter().map(|(_, n)| n).collect();
            out.push((
                table,
                TableMeta {
                    columns,
                    primary_key,
                },
            ));
        }

        Ok(out)
    }

    /// Load indexes and foreign keys for a single table (lazy, on expand).
    pub async fn introspect_table_detail(
        &self,
        driver: Driver,
        schema: &str,
        table: &str,
    ) -> Result<TableDetail, SextantError> {
        match driver {
            Driver::Postgres => self.detail_postgres(schema, table).await,
            Driver::Mysql => self.detail_mysql(schema, table).await,
            Driver::Sqlite => self.detail_sqlite(schema, table).await,
        }
    }

    async fn detail_postgres(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<TableDetail, SextantError> {
        let DbPool::Postgres(pool) = self.pool() else {
            return Err(SextantError::Database("expected postgres pool".to_string()));
        };

        let idx_rows = sqlx::query::<sqlx::Postgres>(
            "SELECT i.relname AS index_name, ix.indisunique AS is_unique, a.attname AS column_name \
             FROM pg_class t \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN pg_index ix ON ix.indrelid = t.oid \
             JOIN pg_class i ON i.oid = ix.indexrelid \
             JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ANY(ix.indkey) \
             WHERE n.nspname = $1 AND t.relname = $2 \
             ORDER BY i.relname, array_position(ix.indkey::int[], a.attnum)",
        )
        .bind(schema)
        .bind(table)
        .fetch_all(pool)
        .await
        .map_err(|e| SextantError::Database(format!("failed to list indexes: {e}")))?;

        let mut indexes: Vec<IndexMeta> = Vec::new();
        for row in &idx_rows {
            let name: String = row
                .try_get("index_name")
                .map_err(|e| SextantError::Database(format!("failed to read index: {e}")))?;
            let unique: bool = row.try_get("is_unique").unwrap_or(false);
            let column: String = row
                .try_get("column_name")
                .map_err(|e| SextantError::Database(format!("failed to read index col: {e}")))?;
            append_index(&mut indexes, name, column, unique);
        }

        let fk_rows = sqlx::query::<sqlx::Postgres>(
            "SELECT tc.constraint_name, kcu.column_name, ccu.table_name AS ref_table, \
                    ccu.column_name AS ref_column \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON tc.constraint_name = kcu.constraint_name \
              AND tc.table_schema = kcu.table_schema \
             JOIN information_schema.constraint_column_usage ccu \
               ON ccu.constraint_name = tc.constraint_name \
              AND ccu.table_schema = tc.table_schema \
             WHERE tc.constraint_type = 'FOREIGN KEY' \
               AND tc.table_schema = $1 AND tc.table_name = $2 \
             ORDER BY tc.constraint_name",
        )
        .bind(schema)
        .bind(table)
        .fetch_all(pool)
        .await
        .map_err(|e| SextantError::Database(format!("failed to list foreign keys: {e}")))?;

        let mut foreign_keys: Vec<ForeignKeyMeta> = Vec::new();
        for row in &fk_rows {
            append_fk(
                &mut foreign_keys,
                read_str(row, "constraint_name")?,
                read_str(row, "column_name")?,
                read_str(row, "ref_table")?,
                read_str(row, "ref_column")?,
            );
        }

        Ok(TableDetail {
            indexes,
            foreign_keys,
        })
    }

    async fn detail_mysql(&self, schema: &str, table: &str) -> Result<TableDetail, SextantError> {
        let DbPool::MySql(pool) = self.pool() else {
            return Err(SextantError::Database("expected mysql pool".to_string()));
        };

        let idx_rows = sqlx::query::<sqlx::MySql>(
            "SELECT index_name AS `index_name`, non_unique AS `non_unique`, \
                    column_name AS `column_name` \
             FROM information_schema.statistics \
             WHERE table_schema = ? AND table_name = ? \
             ORDER BY index_name, seq_in_index",
        )
        .bind(schema)
        .bind(table)
        .fetch_all(pool)
        .await
        .map_err(|e| SextantError::Database(format!("failed to list indexes: {e}")))?;

        let mut indexes: Vec<IndexMeta> = Vec::new();
        for row in &idx_rows {
            let non_unique: i32 = row.try_get("non_unique").unwrap_or(1);
            append_index(
                &mut indexes,
                read_str(row, "index_name")?,
                read_str(row, "column_name")?,
                non_unique == 0,
            );
        }

        let fk_rows = sqlx::query::<sqlx::MySql>(
            "SELECT constraint_name AS `constraint_name`, column_name AS `column_name`, \
                    referenced_table_name AS `ref_table`, referenced_column_name AS `ref_column` \
             FROM information_schema.key_column_usage \
             WHERE table_schema = ? AND table_name = ? AND referenced_table_name IS NOT NULL \
             ORDER BY constraint_name, ordinal_position",
        )
        .bind(schema)
        .bind(table)
        .fetch_all(pool)
        .await
        .map_err(|e| SextantError::Database(format!("failed to list foreign keys: {e}")))?;

        let mut foreign_keys: Vec<ForeignKeyMeta> = Vec::new();
        for row in &fk_rows {
            append_fk(
                &mut foreign_keys,
                read_str(row, "constraint_name")?,
                read_str(row, "column_name")?,
                read_str(row, "ref_table")?,
                read_str(row, "ref_column")?,
            );
        }

        Ok(TableDetail {
            indexes,
            foreign_keys,
        })
    }

    async fn detail_sqlite(&self, schema: &str, table: &str) -> Result<TableDetail, SextantError> {
        let DbPool::Sqlite(pool) = self.pool() else {
            return Err(SextantError::Database("expected sqlite pool".to_string()));
        };

        let list_sql = format!("PRAGMA \"{schema}\".index_list(\"{table}\")");
        let list_rows = sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(list_sql))
            .fetch_all(pool)
            .await
            .map_err(|e| SextantError::Database(format!("failed to list indexes: {e}")))?;

        let mut indexes: Vec<IndexMeta> = Vec::new();
        for row in &list_rows {
            let name: String = row
                .try_get("name")
                .map_err(|e| SextantError::Database(format!("failed to read index: {e}")))?;
            let unique: i64 = row.try_get("unique").unwrap_or(0);

            let info_sql = format!("PRAGMA \"{schema}\".index_info(\"{name}\")");
            let info_rows = sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(info_sql))
                .fetch_all(pool)
                .await
                .map_err(|e| SextantError::Database(format!("failed to read index_info: {e}")))?;
            let columns = info_rows
                .iter()
                .filter_map(|r| r.try_get::<Option<String>, _>("name").ok().flatten())
                .collect();

            indexes.push(IndexMeta {
                name,
                columns,
                unique: unique != 0,
            });
        }

        let fk_sql = format!("PRAGMA \"{schema}\".foreign_key_list(\"{table}\")");
        let fk_rows = sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(fk_sql))
            .fetch_all(pool)
            .await
            .map_err(|e| SextantError::Database(format!("failed to list foreign keys: {e}")))?;

        let mut foreign_keys: Vec<ForeignKeyMeta> = Vec::new();
        let mut current_id: Option<i64> = None;
        for row in &fk_rows {
            let id: i64 = row.try_get("id").unwrap_or(0);
            let ref_table = read_str(row, "table")?;
            let from = read_str(row, "from")?;
            let to = row
                .try_get::<Option<String>, _>("to")
                .ok()
                .flatten()
                .unwrap_or_default();
            if current_id == Some(id) {
                if let Some(last) = foreign_keys.last_mut() {
                    last.columns.push(from);
                    last.ref_columns.push(to);
                }
            } else {
                current_id = Some(id);
                foreign_keys.push(ForeignKeyMeta {
                    name: format!("fk_{id}"),
                    columns: vec![from],
                    ref_table,
                    ref_columns: vec![to],
                });
            }
        }

        Ok(TableDetail {
            indexes,
            foreign_keys,
        })
    }
}

/// Read a required string column, mapping decode errors to `SextantError`.
fn read_str<R: Row>(row: &R, name: &str) -> Result<String, SextantError>
where
    for<'a> String: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> &'a str: sqlx::ColumnIndex<R>,
{
    row.try_get(name)
        .map_err(|e| SextantError::Database(format!("failed to read {name}: {e}")))
}

/// Append an index row, grouping consecutive rows that share an index name.
fn append_index(indexes: &mut Vec<IndexMeta>, name: String, column: String, unique: bool) {
    if let Some(last) = indexes.last_mut() {
        if last.name == name {
            last.columns.push(column);
            return;
        }
    }
    indexes.push(IndexMeta {
        name,
        columns: vec![column],
        unique,
    });
}

/// Append an FK row, grouping consecutive rows that share a constraint name.
fn append_fk(
    fks: &mut Vec<ForeignKeyMeta>,
    name: String,
    column: String,
    ref_table: String,
    ref_column: String,
) {
    if let Some(last) = fks.last_mut() {
        if last.name == name {
            last.columns.push(column);
            last.ref_columns.push(ref_column);
            return;
        }
    }
    fks.push(ForeignKeyMeta {
        name,
        columns: vec![column],
        ref_table,
        ref_columns: vec![ref_column],
    });
}

/// Merge a per-table column map with a per-table primary-key map, marking PK
/// columns and attaching the ordered PK list. Used by the PG/MySQL paths where
/// columns and primary keys are gathered separately.
fn merge_columns(
    cols_map: BTreeMap<String, Vec<ColumnMeta>>,
    mut pk_map: BTreeMap<String, Vec<String>>,
) -> Vec<(String, TableMeta)> {
    cols_map
        .into_iter()
        .map(|(table, mut columns)| {
            let primary_key = pk_map.remove(&table).unwrap_or_default();
            for c in &mut columns {
                c.is_primary_key = primary_key.contains(&c.name);
            }
            (
                table,
                TableMeta {
                    columns,
                    primary_key,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sextant_core::QueryExecutor;

    async fn sqlite_executor() -> SqlxExecutor {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        SqlxExecutor::new(DbPool::Sqlite(pool))
    }

    #[tokio::test]
    async fn sqlite_introspection() {
        let exec = sqlite_executor().await;

        exec.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)")
            .await
            .unwrap();
        exec.execute("CREATE TABLE orders (id INTEGER PRIMARY KEY)")
            .await
            .unwrap();

        let schemas = exec
            .introspect_schemas_and_tables(Driver::Sqlite)
            .await
            .unwrap();

        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "main");
        assert_eq!(schemas[0].tables, vec!["orders", "users"]);
    }

    #[tokio::test]
    async fn sqlite_empty_database() {
        let exec = sqlite_executor().await;

        let schemas = exec
            .introspect_schemas_and_tables(Driver::Sqlite)
            .await
            .unwrap();

        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "main");
        assert!(schemas[0].tables.is_empty());
    }

    #[tokio::test]
    async fn postgres_not_available_at_runtime() {
        // This test just verifies the function path exists for Postgres.
        // Real PG testing requires a running server (see executor tests).
        let exec = sqlite_executor().await;
        let result = exec.introspect_schemas_and_tables(Driver::Postgres).await;
        // Will fail because sqlite doesn't have information_schema.schemata,
        // but we verify the code path compiles and runs.
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mysql_not_available_at_runtime() {
        // This test just verifies the function path exists for MySQL.
        let exec = sqlite_executor().await;
        let result = exec.introspect_schemas_and_tables(Driver::Mysql).await;
        // Will fail because sqlite doesn't have the expected MySQL pool,
        // but we verify the code path compiles and runs.
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn sqlite_introspect_columns() {
        let exec = sqlite_executor().await;
        exec.execute(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT DEFAULT 'x')",
        )
        .await
        .unwrap();

        let tables = exec
            .introspect_columns(Driver::Sqlite, "main")
            .await
            .unwrap();
        assert_eq!(tables.len(), 1);

        let (table, meta) = &tables[0];
        assert_eq!(table, "users");
        assert_eq!(meta.primary_key, vec!["id"]);
        assert_eq!(meta.columns.len(), 3);

        assert_eq!(meta.columns[0].name, "id");
        assert!(meta.columns[0].is_primary_key);

        assert_eq!(meta.columns[1].name, "name");
        assert!(!meta.columns[1].nullable);
        assert!(!meta.columns[1].is_primary_key);

        assert_eq!(meta.columns[2].name, "email");
        assert!(meta.columns[2].nullable);
        assert_eq!(meta.columns[2].default.as_deref(), Some("'x'"));
    }

    #[tokio::test]
    async fn sqlite_introspect_composite_pk_ordered() {
        let exec = sqlite_executor().await;
        exec.execute(
            "CREATE TABLE membership (a INTEGER, b INTEGER, val TEXT, PRIMARY KEY (a, b))",
        )
        .await
        .unwrap();

        let tables = exec
            .introspect_columns(Driver::Sqlite, "main")
            .await
            .unwrap();
        let (_, meta) = &tables[0];
        assert_eq!(meta.primary_key, vec!["a", "b"]);
        assert!(
            !meta
                .columns
                .iter()
                .find(|c| c.name == "val")
                .unwrap()
                .is_primary_key
        );
    }

    #[tokio::test]
    async fn sqlite_introspect_columns_no_pk() {
        let exec = sqlite_executor().await;
        exec.execute("CREATE TABLE logs (msg TEXT)").await.unwrap();

        let tables = exec
            .introspect_columns(Driver::Sqlite, "main")
            .await
            .unwrap();
        let (_, meta) = &tables[0];
        assert!(meta.primary_key.is_empty());
        assert!(meta.columns.iter().all(|c| !c.is_primary_key));
    }

    #[tokio::test]
    async fn sqlite_table_detail_indexes_and_fks() {
        let exec = sqlite_executor().await;
        exec.execute("CREATE TABLE dept (id INTEGER PRIMARY KEY)")
            .await
            .unwrap();
        exec.execute(
            "CREATE TABLE emp (id INTEGER PRIMARY KEY, dept_id INTEGER, name TEXT, \
             FOREIGN KEY (dept_id) REFERENCES dept(id))",
        )
        .await
        .unwrap();
        exec.execute("CREATE UNIQUE INDEX idx_emp_name ON emp(name)")
            .await
            .unwrap();

        let detail = exec
            .introspect_table_detail(Driver::Sqlite, "main", "emp")
            .await
            .unwrap();

        let idx = detail
            .indexes
            .iter()
            .find(|i| i.name == "idx_emp_name")
            .expect("idx_emp_name present");
        assert_eq!(idx.columns, vec!["name"]);
        assert!(idx.unique);

        assert_eq!(detail.foreign_keys.len(), 1);
        let fk = &detail.foreign_keys[0];
        assert_eq!(fk.columns, vec!["dept_id"]);
        assert_eq!(fk.ref_table, "dept");
        assert_eq!(fk.ref_columns, vec!["id"]);
    }
}
