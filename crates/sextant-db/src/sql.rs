//! SQL text generation helpers: identifier quoting and DDL skeletons.

use sextant_core::Driver;

use crate::introspection::TableMeta;

/// Quote an identifier for the given dialect.
///
/// MySQL uses backticks; PostgreSQL and SQLite use double quotes. Embedded
/// quote characters are doubled so the result is always a single safe token.
pub fn quote_ident(driver: Driver, name: &str) -> String {
    match driver {
        Driver::Mysql => format!("`{}`", name.replace('`', "``")),
        Driver::Postgres | Driver::Sqlite => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// Build a schema-qualified, quoted table reference (e.g. `"public"."users"`).
///
/// When `schema` is empty the table name is returned on its own.
pub fn qualified_table(driver: Driver, schema: &str, table: &str) -> String {
    if schema.is_empty() {
        quote_ident(driver, table)
    } else {
        format!(
            "{}.{}",
            quote_ident(driver, schema),
            quote_ident(driver, table)
        )
    }
}

/// Generate a `CREATE TABLE` skeleton from cached column metadata.
///
/// This is a *skeleton* meant to be emitted into the editor for the user to
/// refine, not an exact round-trip of the original DDL: it carries column
/// names, declared types, `NOT NULL`, defaults and the primary key.
pub fn generate_create_table(
    driver: Driver,
    schema: &str,
    table: &str,
    meta: &TableMeta,
) -> String {
    let mut lines: Vec<String> = Vec::new();

    for col in &meta.columns {
        let mut parts = vec![quote_ident(driver, &col.name), col.type_name.clone()];
        if !col.nullable {
            parts.push("NOT NULL".to_string());
        }
        if let Some(default) = &col.default {
            parts.push(format!("DEFAULT {default}"));
        }
        lines.push(format!("    {}", parts.join(" ")));
    }

    if !meta.primary_key.is_empty() {
        let pk: Vec<String> = meta
            .primary_key
            .iter()
            .map(|c| quote_ident(driver, c))
            .collect();
        lines.push(format!("    PRIMARY KEY ({})", pk.join(", ")));
    }

    format!(
        "CREATE TABLE {} (\n{}\n);",
        qualified_table(driver, schema, table),
        lines.join(",\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::introspection::ColumnMeta;

    fn col(name: &str, type_name: &str, nullable: bool, is_pk: bool) -> ColumnMeta {
        ColumnMeta {
            name: name.to_string(),
            type_name: type_name.to_string(),
            nullable,
            default: None,
            is_primary_key: is_pk,
        }
    }

    #[test]
    fn quote_ident_per_dialect() {
        assert_eq!(quote_ident(Driver::Postgres, "users"), "\"users\"");
        assert_eq!(quote_ident(Driver::Sqlite, "users"), "\"users\"");
        assert_eq!(quote_ident(Driver::Mysql, "users"), "`users`");
    }

    #[test]
    fn quote_ident_escapes_embedded_quotes() {
        assert_eq!(quote_ident(Driver::Postgres, "we\"ird"), "\"we\"\"ird\"");
        assert_eq!(quote_ident(Driver::Mysql, "we`ird"), "`we``ird`");
    }

    #[test]
    fn qualified_table_with_and_without_schema() {
        assert_eq!(
            qualified_table(Driver::Postgres, "public", "users"),
            "\"public\".\"users\""
        );
        assert_eq!(qualified_table(Driver::Postgres, "", "users"), "\"users\"");
    }

    #[test]
    fn create_table_with_pk_and_not_null() {
        let meta = TableMeta {
            columns: vec![
                col("id", "integer", false, true),
                col("name", "text", false, false),
                col("note", "text", true, false),
            ],
            primary_key: vec!["id".to_string()],
        };
        let ddl = generate_create_table(Driver::Postgres, "public", "users", &meta);
        let expected = "CREATE TABLE \"public\".\"users\" (\n    \"id\" integer NOT NULL,\n    \"name\" text NOT NULL,\n    \"note\" text,\n    PRIMARY KEY (\"id\")\n);";
        assert_eq!(ddl, expected);
    }

    #[test]
    fn create_table_composite_pk_and_default() {
        let mut a = col("a", "INTEGER", false, true);
        a.default = Some("0".to_string());
        let meta = TableMeta {
            columns: vec![
                a,
                col("b", "INTEGER", false, true),
                col("val", "TEXT", true, false),
            ],
            primary_key: vec!["a".to_string(), "b".to_string()],
        };
        let ddl = generate_create_table(Driver::Sqlite, "main", "membership", &meta);
        assert!(ddl.contains("\"a\" INTEGER NOT NULL DEFAULT 0"));
        assert!(ddl.contains("PRIMARY KEY (\"a\", \"b\")"));
        assert!(ddl.starts_with("CREATE TABLE \"main\".\"membership\" ("));
    }

    #[test]
    fn create_table_no_pk_omits_pk_clause() {
        let meta = TableMeta {
            columns: vec![col("msg", "text", true, false)],
            primary_key: vec![],
        };
        let ddl = generate_create_table(Driver::Mysql, "app", "logs", &meta);
        assert!(!ddl.contains("PRIMARY KEY"));
        assert!(ddl.contains("CREATE TABLE `app`.`logs`"));
        assert!(ddl.contains("`msg` text"));
    }
}
