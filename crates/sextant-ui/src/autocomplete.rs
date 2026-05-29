//! Lightweight SQL autocomplete: context heuristics over a token scan.
//!
//! This deliberately avoids a full SQL parser (`sqlparser` chokes on the
//! incomplete SQL typed mid-edit). It inspects the text *before* the cursor to
//! decide what to offer, and the full buffer to resolve table aliases.

use std::collections::HashMap;

/// SQL keywords and common functions offered when no table/column context fits.
const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "FULL",
    "CROSS",
    "ON",
    "GROUP",
    "BY",
    "ORDER",
    "LIMIT",
    "OFFSET",
    "HAVING",
    "DISTINCT",
    "AS",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS",
    "IN",
    "LIKE",
    "BETWEEN",
    "EXISTS",
    "UNION",
    "ALL",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "ASC",
    "DESC",
    "CREATE",
    "TABLE",
    "ALTER",
    "DROP",
    "INDEX",
    "VIEW",
    "PRIMARY",
    "KEY",
    "FOREIGN",
    "REFERENCES",
    "DEFAULT",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "COALESCE",
    "NOW",
    "CAST",
];

/// Maximum number of candidates returned for a single completion request.
const MAX_CANDIDATES: usize = 12;

/// Table and column names available for completion on the active connection.
#[derive(Debug, Clone, Default)]
pub struct SchemaIndex {
    /// Table names in their canonical case.
    pub tables: Vec<String>,
    /// Lowercased table name -> column names.
    columns: HashMap<String, Vec<String>>,
}

impl SchemaIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a table and its columns.
    pub fn add_table(&mut self, table: String, columns: Vec<String>) {
        self.columns.insert(table.to_lowercase(), columns);
        if !self.tables.iter().any(|t| t == &table) {
            self.tables.push(table);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    fn columns_for(&self, table: &str) -> Vec<String> {
        self.columns
            .get(&table.to_lowercase())
            .cloned()
            .unwrap_or_default()
    }
}

/// The identifier currently being typed: the trailing run of `[A-Za-z0-9_]`.
pub fn current_prefix(before: &str) -> String {
    let mut start = before.len();
    for (i, c) in before.char_indices().rev() {
        if c.is_alphanumeric() || c == '_' {
            start = i;
        } else {
            break;
        }
    }
    before[start..].to_string()
}

/// Compute completion candidates for the cursor position.
///
/// `before` is the buffer text up to the cursor; `full` is the whole buffer
/// (used to resolve aliases that may be declared after the cursor, e.g.
/// `SELECT u.| FROM users u`).
pub fn complete(full: &str, before: &str, index: &SchemaIndex) -> Vec<String> {
    let prefix = current_prefix(before);
    let before_prefix = &before[..before.len() - prefix.len()];

    if let Some(qualifier) = dotted_qualifier(before_prefix) {
        let table = resolve_table(full, &qualifier);
        return finish(index.columns_for(&table), &prefix);
    }

    match last_word(before_prefix).as_deref() {
        Some("FROM") | Some("JOIN") | Some("INTO") | Some("UPDATE") => {
            finish(index.tables.clone(), &prefix)
        }
        _ => {
            let mut cands: Vec<String> = KEYWORDS.iter().map(|s| s.to_string()).collect();
            cands.extend(index.tables.iter().cloned());
            finish(cands, &prefix)
        }
    }
}

/// If `before_prefix` ends with `<ident>.`, return that identifier.
fn dotted_qualifier(before_prefix: &str) -> Option<String> {
    let without_dot = before_prefix.strip_suffix('.')?;
    let mut start = without_dot.len();
    for (i, c) in without_dot.char_indices().rev() {
        if c.is_alphanumeric() || c == '_' {
            start = i;
        } else {
            break;
        }
    }
    let ident = &without_dot[start..];
    if ident.is_empty() {
        None
    } else {
        Some(ident.to_string())
    }
}

/// Last identifier-like word in `s`, uppercased.
fn last_word(s: &str) -> Option<String> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .rfind(|t| !t.is_empty())
        .map(|t| t.to_uppercase())
}

fn is_keyword(word: &str) -> bool {
    let up = word.to_uppercase();
    KEYWORDS.contains(&up.as_str())
}

/// Resolve an alias (or table name) to a table name by scanning `FROM`/`JOIN`
/// clauses across the whole buffer. Falls back to the qualifier itself.
fn resolve_table(full: &str, qualifier: &str) -> String {
    let toks: Vec<&str> = full
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .collect();

    let mut map: HashMap<String, String> = HashMap::new();
    let mut i = 0;
    while i < toks.len() {
        let up = toks[i].to_uppercase();
        if (up == "FROM" || up == "JOIN") && i + 1 < toks.len() {
            let table = toks[i + 1];
            if !is_keyword(table) {
                map.insert(table.to_lowercase(), table.to_string());
                let mut j = i + 2;
                if toks
                    .get(j)
                    .map(|t| t.eq_ignore_ascii_case("AS"))
                    .unwrap_or(false)
                {
                    j += 1;
                }
                if let Some(alias) = toks.get(j) {
                    if !is_keyword(alias) {
                        map.insert(alias.to_lowercase(), table.to_string());
                    }
                }
            }
        }
        i += 1;
    }

    map.get(&qualifier.to_lowercase())
        .cloned()
        .unwrap_or_else(|| qualifier.to_string())
}

/// Filter candidates by `prefix` (case-insensitive), sort, dedupe and cap.
fn finish(mut candidates: Vec<String>, prefix: &str) -> Vec<String> {
    let p = prefix.to_lowercase();
    candidates.retain(|c| c.to_lowercase().starts_with(&p));
    candidates.sort();
    candidates.dedup();
    candidates.truncate(MAX_CANDIDATES);
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> SchemaIndex {
        let mut idx = SchemaIndex::new();
        idx.add_table(
            "users".into(),
            vec!["id".into(), "name".into(), "email".into()],
        );
        idx.add_table("orders".into(), vec!["id".into(), "user_id".into()]);
        idx
    }

    #[test]
    fn prefix_extraction() {
        assert_eq!(current_prefix("SELECT * FROM us"), "us");
        assert_eq!(current_prefix("SELECT * FROM "), "");
        assert_eq!(current_prefix("a.b"), "b");
    }

    #[test]
    fn after_from_offers_tables() {
        let sql = "SELECT * FROM ";
        let c = complete(sql, sql, &index());
        assert_eq!(c, vec!["orders".to_string(), "users".to_string()]);
    }

    #[test]
    fn after_from_filters_by_prefix() {
        let sql = "SELECT * FROM us";
        let c = complete(sql, sql, &index());
        assert_eq!(c, vec!["users".to_string()]);
    }

    #[test]
    fn dotted_table_offers_columns() {
        let sql = "SELECT users. FROM users";
        let before = "SELECT users.";
        let c = complete(sql, before, &index());
        assert_eq!(
            c,
            vec!["email".to_string(), "id".to_string(), "name".to_string()]
        );
    }

    #[test]
    fn dotted_column_filters_by_prefix() {
        let sql = "SELECT users.na FROM users";
        let before = "SELECT users.na";
        let c = complete(sql, before, &index());
        assert_eq!(c, vec!["name".to_string()]);
    }

    #[test]
    fn alias_resolves_to_table_columns() {
        // Alias is declared *after* the cursor — must still resolve.
        let full = "SELECT u. FROM users u";
        let before = "SELECT u.";
        let c = complete(full, before, &index());
        assert_eq!(
            c,
            vec!["email".to_string(), "id".to_string(), "name".to_string()]
        );
    }

    #[test]
    fn alias_with_as_keyword() {
        let full = "SELECT o.user_id FROM orders AS o";
        let before = "SELECT o.";
        let c = complete(full, before, &index());
        assert_eq!(c, vec!["id".to_string(), "user_id".to_string()]);
    }

    #[test]
    fn keyword_context_offers_keywords_and_tables() {
        let sql = "SEL";
        let c = complete(sql, sql, &index());
        assert!(c.contains(&"SELECT".to_string()));
    }

    #[test]
    fn unknown_qualifier_yields_no_columns() {
        let sql = "SELECT bogus.";
        let c = complete(sql, sql, &index());
        assert!(c.is_empty());
    }
}
