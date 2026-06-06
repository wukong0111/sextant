//! Parse CSV/JSON/SQL files and turn them into statements for a target table.
//!
//! Like [`crate::export`], the parsing and statement generation here are pure
//! (no I/O): the caller reads the file and runs the produced statements in a
//! transaction. CSV and JSON are matched to an existing table's columns *by
//! name*; a SQL file is split into independent statements and run as-is.

use sextant_core::Driver;

use crate::introspection::{ColumnMeta, TableMeta};
use crate::sql::{qualified_table, quote_ident, to_sql_literal};

/// Tabular data parsed from a CSV or JSON file (every value as a string).
///
/// Each row is aligned to `columns`; a short row is padded with empty strings,
/// which import treats as SQL `NULL`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportData {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// How source columns line up with a target table's columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMapping {
    /// `(source column index, target column name)` for every matched column.
    pub pairs: Vec<(usize, String)>,
    /// Source columns with no column of the same name in the target table.
    pub unmatched_source: Vec<String>,
    /// Target columns absent from the source (left to their default / NULL).
    pub unmatched_target: Vec<String>,
}

/// A read-only summary shown before an import is run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPreview {
    pub row_count: usize,
    pub mapping: ColumnMapping,
    /// Number of cell values that don't parse for their target column type.
    pub type_issues: usize,
}

/// Coarse classification of a SQL type, derived from its declared name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeClass {
    Integer,
    Float,
    Bool,
    Text,
}

/// Parse `text` as RFC 4180 CSV; the first record is the header row.
pub fn parse_csv(text: &str) -> Result<ImportData, String> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(text.as_bytes());
    let columns: Vec<String> = reader
        .headers()
        .map_err(|e| format!("invalid CSV header: {e}"))?
        .iter()
        .map(|s| s.to_string())
        .collect();
    if columns.is_empty() {
        return Err("CSV has no header row".into());
    }
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| format!("invalid CSV row: {e}"))?;
        let mut row: Vec<String> = record.iter().map(|s| s.to_string()).collect();
        row.resize(columns.len(), String::new());
        rows.push(row);
    }
    Ok(ImportData { columns, rows })
}

/// Parse `text` as a JSON array of objects.
///
/// Columns are the union of all object keys in first-seen order; a missing key
/// becomes an empty string (SQL `NULL`). Non-string scalars are stringified;
/// nested arrays/objects are kept as compact JSON.
pub fn parse_json(text: &str) -> Result<ImportData, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    let serde_json::Value::Array(items) = value else {
        return Err("JSON import expects an array of objects".into());
    };

    let mut columns: Vec<String> = Vec::new();
    let mut objects: Vec<serde_json::Map<String, serde_json::Value>> = Vec::new();
    for item in items {
        let serde_json::Value::Object(map) = item else {
            return Err("every JSON array element must be an object".into());
        };
        for key in map.keys() {
            if !columns.iter().any(|c| c == key) {
                columns.push(key.clone());
            }
        }
        objects.push(map);
    }
    if columns.is_empty() {
        return Err("JSON array has no object keys".into());
    }

    let rows = objects
        .iter()
        .map(|obj| {
            columns
                .iter()
                .map(|col| obj.get(col).map(json_scalar).unwrap_or_default())
                .collect()
        })
        .collect();
    Ok(ImportData { columns, rows })
}

/// Stringify a JSON value for a table cell.
fn json_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        // Nested arrays/objects: keep the compact JSON text.
        other => other.to_string(),
    }
}

/// Split a SQL script into individual statements on top-level semicolons.
///
/// Quoting is honored so a `;` inside a `'...'` string literal does not split a
/// statement; an escaped `''` quote keeps the string open. Empty fragments
/// (e.g. trailing whitespace after the last `;`) are dropped.
pub fn split_sql_statements(text: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' if in_quote && chars.peek() == Some(&'\'') => {
                // Escaped quote inside a string: consume both, stay in-string.
                current.push('\'');
                current.push('\'');
                chars.next();
            }
            '\'' => {
                in_quote = !in_quote;
                current.push(c);
            }
            ';' if !in_quote => {
                if !current.trim().is_empty() {
                    statements.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        statements.push(current.trim().to_string());
    }
    statements
}

/// Match source columns to the target table's columns by name (case-insensitive).
pub fn match_columns(source: &[String], meta: &TableMeta) -> ColumnMapping {
    let mut pairs = Vec::new();
    let mut unmatched_source = Vec::new();
    for (idx, name) in source.iter().enumerate() {
        match meta
            .columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
        {
            Some(col) => pairs.push((idx, col.name.clone())),
            None => unmatched_source.push(name.clone()),
        }
    }
    let unmatched_target = meta
        .columns
        .iter()
        .filter(|c| !pairs.iter().any(|(_, t)| t == &c.name))
        .map(|c| c.name.clone())
        .collect();
    ColumnMapping {
        pairs,
        unmatched_source,
        unmatched_target,
    }
}

/// Build a read-only [`ImportPreview`]: column mapping plus a count of values
/// that don't parse for their target column type.
pub fn preview(data: &ImportData, meta: &TableMeta) -> ImportPreview {
    let mapping = match_columns(&data.columns, meta);
    let mut type_issues = 0;
    for row in &data.rows {
        for (src_idx, target) in &mapping.pairs {
            let class = target_class(meta, target);
            let value = row.get(*src_idx).map(String::as_str).unwrap_or("");
            if !value_parses(value, class) {
                type_issues += 1;
            }
        }
    }
    ImportPreview {
        row_count: data.rows.len(),
        mapping,
        type_issues,
    }
}

/// Build one `INSERT` per row, filling only the matched (mapped) columns.
pub fn build_inserts(
    driver: Driver,
    schema: &str,
    table: &str,
    data: &ImportData,
    mapping: &ColumnMapping,
    meta: &TableMeta,
) -> Vec<String> {
    if mapping.pairs.is_empty() {
        return Vec::new();
    }
    let table_ref = qualified_table(driver, schema, table);
    let cols = mapping
        .pairs
        .iter()
        .map(|(_, target)| quote_ident(driver, target))
        .collect::<Vec<_>>()
        .join(", ");
    data.rows
        .iter()
        .map(|row| {
            let values = mapping
                .pairs
                .iter()
                .map(|(src_idx, target)| {
                    let value = row.get(*src_idx).map(String::as_str).unwrap_or("");
                    value_to_literal(value, target_class(meta, target))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("INSERT INTO {table_ref} ({cols}) VALUES ({values});")
        })
        .collect()
}

/// Classification of the target column named `target` (defaults to text).
fn target_class(meta: &TableMeta, target: &str) -> TypeClass {
    meta.columns
        .iter()
        .find(|c| c.name == target)
        .map(classify)
        .unwrap_or(TypeClass::Text)
}

fn classify(col: &ColumnMeta) -> TypeClass {
    let t = col.type_name.to_lowercase();
    if t.contains("int") {
        TypeClass::Integer
    } else if t.contains("real")
        || t.contains("floa")
        || t.contains("doub")
        || t.contains("numeric")
        || t.contains("decimal")
    {
        TypeClass::Float
    } else if t.contains("bool") {
        TypeClass::Bool
    } else {
        TypeClass::Text
    }
}

/// Whether `value` is acceptable for a column of `class` (empty = NULL = ok).
fn value_parses(value: &str, class: TypeClass) -> bool {
    if value.is_empty() {
        return true;
    }
    match class {
        TypeClass::Integer => value.parse::<i64>().is_ok(),
        TypeClass::Float => value.parse::<f64>().is_ok(),
        TypeClass::Bool => parse_bool(value).is_some(),
        TypeClass::Text => true,
    }
}

/// Render `value` as a SQL literal for a column of `class`.
///
/// An empty string becomes `NULL`. Numbers that parse are emitted unquoted;
/// recognizable booleans become `TRUE`/`FALSE`; everything else is a quoted,
/// escaped string literal (which the backends coerce as needed).
fn value_to_literal(value: &str, class: TypeClass) -> String {
    if value.is_empty() {
        return "NULL".to_string();
    }
    match class {
        TypeClass::Integer if value.parse::<i64>().is_ok() => value.to_string(),
        TypeClass::Float if value.parse::<f64>().is_ok() => value.to_string(),
        TypeClass::Bool => match parse_bool(value) {
            Some(true) => "TRUE".to_string(),
            Some(false) => "FALSE".to_string(),
            None => to_sql_literal(value),
        },
        _ => to_sql_literal(value),
    }
}

/// Parse a permissive boolean (`true/false`, `t/f`, `1/0`, `yes/no`).
fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "true" | "t" | "1" | "yes" | "y" => Some(true),
        "false" | "f" | "0" | "no" | "n" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> TableMeta {
        TableMeta {
            columns: vec![
                ColumnMeta {
                    name: "id".into(),
                    type_name: "INTEGER".into(),
                    nullable: false,
                    default: None,
                    is_primary_key: true,
                },
                ColumnMeta {
                    name: "name".into(),
                    type_name: "TEXT".into(),
                    nullable: true,
                    default: None,
                    is_primary_key: false,
                },
            ],
            primary_key: vec!["id".into()],
        }
    }

    #[test]
    fn csv_parses_header_and_rows() {
        let data = parse_csv("id,name\n1,alice\n2,bob\n").unwrap();
        assert_eq!(data.columns, vec!["id", "name"]);
        assert_eq!(data.rows.len(), 2);
        assert_eq!(data.rows[0], vec!["1", "alice"]);
    }

    #[test]
    fn csv_pads_short_rows() {
        let data = parse_csv("id,name\n1\n").unwrap();
        assert_eq!(data.rows[0], vec!["1", ""]);
    }

    #[test]
    fn json_unions_keys_and_stringifies_scalars() {
        let data = parse_json(r#"[{"id": 1, "name": "alice"}, {"id": 2}]"#).unwrap();
        assert_eq!(data.columns, vec!["id", "name"]);
        assert_eq!(data.rows[0], vec!["1", "alice"]);
        // Missing key -> empty (NULL).
        assert_eq!(data.rows[1], vec!["2", ""]);
    }

    #[test]
    fn json_rejects_non_array() {
        assert!(parse_json(r#"{"id": 1}"#).is_err());
    }

    #[test]
    fn split_sql_respects_quoted_semicolons() {
        let stmts =
            split_sql_statements("INSERT INTO t VALUES ('a; b'); DELETE FROM t WHERE x = '';\n");
        assert_eq!(
            stmts,
            vec![
                "INSERT INTO t VALUES ('a; b')",
                "DELETE FROM t WHERE x = ''",
            ]
        );
    }

    #[test]
    fn split_sql_keeps_escaped_quotes_in_string() {
        let stmts = split_sql_statements("INSERT INTO t VALUES ('it''s; ok');");
        assert_eq!(stmts, vec!["INSERT INTO t VALUES ('it''s; ok')"]);
    }

    #[test]
    fn matching_is_case_insensitive_and_reports_unmatched() {
        let data = ImportData {
            columns: vec!["ID".into(), "extra".into()],
            rows: vec![],
        };
        let mapping = match_columns(&data.columns, &meta());
        assert_eq!(mapping.pairs, vec![(0, "id".to_string())]);
        assert_eq!(mapping.unmatched_source, vec!["extra"]);
        assert_eq!(mapping.unmatched_target, vec!["name"]);
    }

    #[test]
    fn preview_counts_type_issues() {
        let data = parse_csv("id,name\n1,alice\nNaN,bob\n").unwrap();
        let pv = preview(&data, &meta());
        assert_eq!(pv.row_count, 2);
        // "NaN" is not a valid INTEGER for the id column.
        assert_eq!(pv.type_issues, 1);
    }

    #[test]
    fn build_inserts_uses_only_mapped_columns_and_typed_literals() {
        let data = parse_csv("id,name,extra\n1,alice,x\n2,,y\n").unwrap();
        let mapping = match_columns(&data.columns, &meta());
        let stmts = build_inserts(Driver::Sqlite, "main", "users", &data, &mapping, &meta());
        assert_eq!(
            stmts,
            vec![
                // numeric id unquoted, text name quoted, `extra` dropped.
                "INSERT INTO \"main\".\"users\" (\"id\", \"name\") VALUES (1, 'alice');",
                // empty name -> NULL.
                "INSERT INTO \"main\".\"users\" (\"id\", \"name\") VALUES (2, NULL);",
            ]
        );
    }

    #[test]
    fn build_inserts_empty_when_no_columns_match() {
        let data = ImportData {
            columns: vec!["nope".into()],
            rows: vec![vec!["1".into()]],
        };
        let mapping = match_columns(&data.columns, &meta());
        let stmts = build_inserts(Driver::Sqlite, "main", "users", &data, &mapping, &meta());
        assert!(stmts.is_empty());
    }
}
