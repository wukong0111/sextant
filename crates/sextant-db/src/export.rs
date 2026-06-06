//! Export a [`QueryResult`] to CSV, JSON, or SQL `INSERT` statements.
//!
//! These are pure transformations over an in-memory result set; they perform no
//! I/O. The caller (UI) is responsible for choosing a destination path and
//! writing the returned `String` with the project's restrictive permissions.

use sextant_core::{CellValue, Driver, QueryResult};

use crate::sql::quote_ident;

/// The serialization formats offered by the exporter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// RFC 4180 comma-separated values, with a header row.
    Csv,
    /// A JSON array of objects keyed by column name.
    Json,
    /// One `INSERT INTO <table> (...) VALUES (...);` per row.
    Sql,
}

impl ExportFormat {
    /// File extension (without the dot) for this format.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
            ExportFormat::Sql => "sql",
        }
    }

    /// Human-readable label for menus.
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Json => "JSON",
            ExportFormat::Sql => "SQL (INSERT)",
        }
    }
}

/// Serialize `result` in the requested `format`.
///
/// `driver` and `table` are only consulted for [`ExportFormat::Sql`] (to quote
/// identifiers per dialect and name the target table).
pub fn export(result: &QueryResult, format: ExportFormat, driver: Driver, table: &str) -> String {
    match format {
        ExportFormat::Csv => to_csv(result),
        ExportFormat::Json => to_json(result),
        ExportFormat::Sql => to_sql(result, driver, table),
    }
}

/// Render `result` as RFC 4180 CSV with a header row.
///
/// `NULL` cells become empty fields (the CSV convention); binary cells are
/// hex-encoded so the output stays valid UTF-8.
pub fn to_csv(result: &QueryResult) -> String {
    let mut wtr = csv::Writer::from_writer(Vec::new());
    let _ = wtr.write_record(result.columns.iter().map(|c| c.name.as_str()));
    for row in &result.rows {
        let _ = wtr.write_record(row.iter().map(csv_cell));
    }
    // `into_inner` only fails on a flush error of the underlying writer; a
    // `Vec<u8>` never fails, and the bytes are valid UTF-8 by construction.
    let bytes = wtr.into_inner().unwrap_or_default();
    String::from_utf8(bytes).unwrap_or_default()
}

/// Render `result` as a pretty-printed JSON array of row objects.
pub fn to_json(result: &QueryResult) -> String {
    let rows: Vec<serde_json::Map<String, serde_json::Value>> = result
        .rows
        .iter()
        .map(|row| {
            result
                .columns
                .iter()
                .zip(row)
                .map(|(col, val)| (col.name.clone(), json_cell(val)))
                .collect()
        })
        .collect();
    serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
}

/// Render `result` as one `INSERT` statement per row targeting `table`.
pub fn to_sql(result: &QueryResult, driver: Driver, table: &str) -> String {
    if result.rows.is_empty() {
        return String::new();
    }
    let cols = result
        .columns
        .iter()
        .map(|c| quote_ident(driver, &c.name))
        .collect::<Vec<_>>()
        .join(", ");
    let table_ref = quote_ident(driver, table);
    let mut out = String::new();
    for row in &result.rows {
        let values = row.iter().map(sql_cell).collect::<Vec<_>>().join(", ");
        out.push_str(&format!(
            "INSERT INTO {table_ref} ({cols}) VALUES ({values});\n"
        ));
    }
    out
}

/// A cell rendered for a CSV field (`NULL` → empty string).
fn csv_cell(value: &CellValue) -> String {
    match value {
        CellValue::Null => String::new(),
        CellValue::Bool(b) => b.to_string(),
        CellValue::I64(v) => v.to_string(),
        CellValue::F64(v) => v.to_string(),
        CellValue::String(s) => s.clone(),
        CellValue::Bytes(b) => hex(b),
    }
}

/// A cell rendered as a JSON value.
fn json_cell(value: &CellValue) -> serde_json::Value {
    use serde_json::Value;
    match value {
        CellValue::Null => Value::Null,
        CellValue::Bool(b) => Value::Bool(*b),
        CellValue::I64(v) => Value::from(*v),
        // A non-finite float has no JSON representation; fall back to null.
        CellValue::F64(v) => serde_json::Number::from_f64(*v)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        CellValue::String(s) => Value::String(s.clone()),
        CellValue::Bytes(b) => Value::String(hex(b)),
    }
}

/// A cell rendered as a SQL literal (type-aware: numbers and booleans unquoted).
fn sql_cell(value: &CellValue) -> String {
    match value {
        CellValue::Null => "NULL".to_string(),
        CellValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        CellValue::I64(v) => v.to_string(),
        CellValue::F64(v) => v.to_string(),
        CellValue::String(s) => format!("'{}'", s.replace('\'', "''")),
        CellValue::Bytes(b) => format!("'{}'", hex(b)),
    }
}

/// Lowercase hex encoding of raw bytes.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use sextant_core::Column;

    fn sample() -> QueryResult {
        QueryResult {
            columns: vec![
                Column {
                    name: "id".into(),
                    type_name: "int".into(),
                },
                Column {
                    name: "name".into(),
                    type_name: "text".into(),
                },
            ],
            rows: vec![
                vec![CellValue::I64(1), CellValue::String("Alice".into())],
                vec![CellValue::I64(2), CellValue::Null],
            ],
            rows_affected: None,
        }
    }

    #[test]
    fn csv_has_header_and_empty_null() {
        let csv = to_csv(&sample());
        assert_eq!(csv, "id,name\n1,Alice\n2,\n");
    }

    #[test]
    fn csv_quotes_fields_with_commas() {
        let result = QueryResult {
            columns: vec![Column {
                name: "v".into(),
                type_name: "text".into(),
            }],
            rows: vec![vec![CellValue::String("a,b".into())]],
            rows_affected: None,
        };
        assert_eq!(to_csv(&result), "v\n\"a,b\"\n");
    }

    #[test]
    fn json_is_array_of_objects_with_typed_values() {
        let json = to_json(&sample());
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0]["id"], serde_json::json!(1));
        assert_eq!(parsed[0]["name"], serde_json::json!("Alice"));
        assert_eq!(parsed[1]["name"], serde_json::Value::Null);
    }

    #[test]
    fn sql_emits_insert_per_row_with_typed_literals() {
        let sql = to_sql(&sample(), Driver::Postgres, "users");
        assert_eq!(
            sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES (1, 'Alice');\n\
             INSERT INTO \"users\" (\"id\", \"name\") VALUES (2, NULL);\n"
        );
    }

    #[test]
    fn sql_escapes_quotes_and_uses_backticks_for_mysql() {
        let result = QueryResult {
            columns: vec![Column {
                name: "v".into(),
                type_name: "text".into(),
            }],
            rows: vec![vec![CellValue::String("it's".into())]],
            rows_affected: None,
        };
        let sql = to_sql(&result, Driver::Mysql, "t");
        assert_eq!(sql, "INSERT INTO `t` (`v`) VALUES ('it''s');\n");
    }

    #[test]
    fn sql_of_empty_result_is_empty() {
        let result = QueryResult {
            columns: vec![Column {
                name: "v".into(),
                type_name: "text".into(),
            }],
            rows: vec![],
            rows_affected: None,
        };
        assert_eq!(to_sql(&result, Driver::Sqlite, "t"), "");
    }
}
