//! Sidebar tree pane for connections, schemas, tables and columns.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem},
};

/// A column node shown under an expanded table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnNode {
    pub name: String,
    pub type_name: String,
    pub is_pk: bool,
}

/// A table inside a schema. Columns come from cached metadata on expand;
/// indexes/FKs are loaded lazily (async) and stored as display strings.
#[derive(Debug, Clone)]
pub struct TableItem {
    pub name: String,
    pub expanded: bool,
    pub columns: Vec<ColumnNode>,
    pub indexes: Vec<String>,
    pub foreign_keys: Vec<String>,
    pub detail_loaded: bool,
}

impl TableItem {
    /// Create a collapsed table with no columns loaded yet.
    pub fn new(name: String) -> Self {
        Self {
            name,
            expanded: false,
            columns: Vec::new(),
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
            detail_loaded: false,
        }
    }
}

/// A schema with its tables inside the tree.
#[derive(Debug, Clone)]
pub struct SchemaItem {
    pub name: String,
    pub expanded: bool,
    pub tables: Vec<TableItem>,
}

/// Connection state within the tree.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected {
        expanded: bool,
        schemas: Vec<SchemaItem>,
    },
    Error(String),
}

/// One saved connection entry in the tree.
#[derive(Debug, Clone)]
pub struct ConnectionItem {
    pub name: String,
    pub state: ConnState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Connection {
        conn: usize,
    },
    Schema {
        conn: usize,
        schema: usize,
    },
    Table {
        conn: usize,
        schema: usize,
        table: usize,
    },
    Column {
        conn: usize,
        schema: usize,
        table: usize,
        col: usize,
    },
    /// A non-actionable detail line (index or foreign key) under a table.
    Detail,
}

#[derive(Debug, Clone)]
struct Line {
    kind: LineKind,
    text: String,
}

/// Tree pane widget state.
#[derive(Debug)]
pub struct TreePane {
    pub connections: Vec<ConnectionItem>,
    pub selected: usize,
}

impl TreePane {
    /// Create a new tree pane from a list of connection names.
    pub fn new(names: Vec<String>) -> Self {
        Self {
            connections: names
                .into_iter()
                .map(|name| ConnectionItem {
                    name,
                    state: ConnState::Disconnected,
                })
                .collect(),
            selected: 0,
        }
    }

    /// Move selection down.
    pub fn next(&mut self) {
        let max = self.visible_lines().len().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
    }

    /// Move selection up.
    pub fn prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Return the kind of the currently selected line, if any.
    pub fn selected_kind(&self) -> Option<LineKind> {
        self.visible_lines().get(self.selected).map(|l| l.kind)
    }

    /// Toggle expand/collapse on the selected connection, schema or table.
    pub fn toggle_selected(&mut self) {
        let Some(kind) = self.selected_kind() else {
            return;
        };
        match kind {
            LineKind::Connection { conn } => {
                if let Some(c) = self.connections.get_mut(conn) {
                    if let ConnState::Connected { expanded, .. } = &mut c.state {
                        *expanded = !*expanded;
                    }
                }
            }
            LineKind::Schema { conn, schema } => {
                if let Some(s) = self.schema_mut(conn, schema) {
                    s.expanded = !s.expanded;
                }
            }
            LineKind::Table {
                conn,
                schema,
                table,
            } => {
                if let Some(t) = self.table_mut(conn, schema, table) {
                    t.expanded = !t.expanded;
                }
            }
            LineKind::Column { .. } | LineKind::Detail => {}
        }
    }

    /// Mark a connection as connecting.
    pub fn set_connecting(&mut self, conn_idx: usize) {
        if let Some(conn) = self.connections.get_mut(conn_idx) {
            conn.state = ConnState::Connecting;
        }
    }

    /// Mark a connection as connected with its schemas.
    pub fn set_connected(&mut self, conn_idx: usize, schemas: Vec<SchemaItem>) {
        if let Some(conn) = self.connections.get_mut(conn_idx) {
            conn.state = ConnState::Connected {
                expanded: true,
                schemas,
            };
        }
    }

    /// Mark a connection as failed.
    pub fn set_error(&mut self, conn_idx: usize, error: String) {
        if let Some(conn) = self.connections.get_mut(conn_idx) {
            conn.state = ConnState::Error(error);
        }
    }

    /// Find the index of a connection by name.
    pub fn connection_index_by_name(&self, name: &str) -> Option<usize> {
        self.connections.iter().position(|c| c.name == name)
    }

    /// Name of the schema at `(conn, schema)`, if connected.
    pub fn schema_name(&self, conn: usize, schema: usize) -> Option<String> {
        self.schema_ref(conn, schema).map(|s| s.name.clone())
    }

    /// Name of the table at `(conn, schema, table)`, if connected.
    pub fn table_name(&self, conn: usize, schema: usize, table: usize) -> Option<String> {
        self.table_ref(conn, schema, table).map(|t| t.name.clone())
    }

    /// Whether the table at `(conn, schema, table)` is currently expanded.
    pub fn is_table_expanded(&self, conn: usize, schema: usize, table: usize) -> bool {
        self.table_ref(conn, schema, table)
            .map(|t| t.expanded)
            .unwrap_or(false)
    }

    /// Set the expanded flag on a table.
    pub fn set_table_expanded(&mut self, conn: usize, schema: usize, table: usize, expanded: bool) {
        if let Some(t) = self.table_mut(conn, schema, table) {
            t.expanded = expanded;
        }
    }

    /// Whether the table already has its columns loaded.
    pub fn table_has_columns(&self, conn: usize, schema: usize, table: usize) -> bool {
        self.table_ref(conn, schema, table)
            .map(|t| !t.columns.is_empty())
            .unwrap_or(false)
    }

    /// Whether the table's index/FK detail has been loaded.
    pub fn table_detail_loaded(&self, conn: usize, schema: usize, table: usize) -> bool {
        self.table_ref(conn, schema, table)
            .map(|t| t.detail_loaded)
            .unwrap_or(false)
    }

    /// Store the table's index/FK detail (pre-formatted display strings).
    pub fn set_table_detail(
        &mut self,
        conn: usize,
        schema: usize,
        table: usize,
        indexes: Vec<String>,
        foreign_keys: Vec<String>,
    ) {
        if let Some(t) = self.table_mut(conn, schema, table) {
            t.indexes = indexes;
            t.foreign_keys = foreign_keys;
            t.detail_loaded = true;
        }
    }

    /// Replace a table's column nodes (filled from cached metadata on expand).
    pub fn set_table_columns(
        &mut self,
        conn: usize,
        schema: usize,
        table: usize,
        columns: Vec<ColumnNode>,
    ) {
        if let Some(t) = self.table_mut(conn, schema, table) {
            t.columns = columns;
        }
    }

    fn schema_ref(&self, conn: usize, schema: usize) -> Option<&SchemaItem> {
        let ConnState::Connected { schemas, .. } = &self.connections.get(conn)?.state else {
            return None;
        };
        schemas.get(schema)
    }

    fn schema_mut(&mut self, conn: usize, schema: usize) -> Option<&mut SchemaItem> {
        let ConnState::Connected { schemas, .. } = &mut self.connections.get_mut(conn)?.state
        else {
            return None;
        };
        schemas.get_mut(schema)
    }

    fn table_ref(&self, conn: usize, schema: usize, table: usize) -> Option<&TableItem> {
        self.schema_ref(conn, schema)?.tables.get(table)
    }

    fn table_mut(&mut self, conn: usize, schema: usize, table: usize) -> Option<&mut TableItem> {
        self.schema_mut(conn, schema)?.tables.get_mut(table)
    }

    fn visible_lines(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        for (ci, conn) in self.connections.iter().enumerate() {
            let prefix = match &conn.state {
                ConnState::Disconnected => "  ",
                ConnState::Connecting => "◌ ",
                ConnState::Connected {
                    expanded: false, ..
                } => "> ",
                ConnState::Connected { expanded: true, .. } => "v ",
                ConnState::Error(_) => "! ",
            };
            lines.push(Line {
                kind: LineKind::Connection { conn: ci },
                text: format!("{}{}", prefix, conn.name),
            });

            if let ConnState::Connected {
                expanded: true,
                schemas,
            } = &conn.state
            {
                for (si, schema) in schemas.iter().enumerate() {
                    let s_prefix = if schema.expanded { "v" } else { ">" };
                    lines.push(Line {
                        kind: LineKind::Schema {
                            conn: ci,
                            schema: si,
                        },
                        text: format!("  {} {}", s_prefix, schema.name),
                    });
                    if schema.expanded {
                        for (ti, table) in schema.tables.iter().enumerate() {
                            let t_prefix = if table.expanded { "v" } else { ">" };
                            lines.push(Line {
                                kind: LineKind::Table {
                                    conn: ci,
                                    schema: si,
                                    table: ti,
                                },
                                text: format!("    {} {}", t_prefix, table.name),
                            });
                            if table.expanded {
                                for (coli, col) in table.columns.iter().enumerate() {
                                    let pk = if col.is_pk { " PK" } else { "" };
                                    lines.push(Line {
                                        kind: LineKind::Column {
                                            conn: ci,
                                            schema: si,
                                            table: ti,
                                            col: coli,
                                        },
                                        text: format!(
                                            "        {} {}{}",
                                            col.name, col.type_name, pk
                                        ),
                                    });
                                }
                                for idx in &table.indexes {
                                    lines.push(Line {
                                        kind: LineKind::Detail,
                                        text: format!("        {idx}"),
                                    });
                                }
                                for fk in &table.foreign_keys {
                                    lines.push(Line {
                                        kind: LineKind::Detail,
                                        text: format!("        {fk}"),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        lines
    }

    /// Render the tree pane into the given area.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let lines = self.visible_lines();
        let items: Vec<ListItem> = lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let style = if i == self.selected {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };
                ListItem::new(line.text.clone()).style(style)
            })
            .collect();

        let list = List::new(items).block(Block::default().borders(Borders::RIGHT));
        frame.render_widget(list, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(names: &[&str]) -> Vec<TableItem> {
        names
            .iter()
            .map(|n| TableItem::new((*n).to_string()))
            .collect()
    }

    #[test]
    fn new_tree_pane() {
        let tree = TreePane::new(vec!["local-pg".into(), "sqlite-dev".into()]);
        assert_eq!(tree.connections.len(), 2);
        assert_eq!(tree.selected, 0);
        assert!(matches!(tree.connections[0].state, ConnState::Disconnected));
    }

    #[test]
    fn navigation_bounds() {
        let mut tree = TreePane::new(vec!["a".into(), "b".into()]);
        assert_eq!(tree.selected, 0);
        tree.next();
        assert_eq!(tree.selected, 1);
        tree.next();
        assert_eq!(tree.selected, 1); // clamped
        tree.prev();
        assert_eq!(tree.selected, 0);
        tree.prev();
        assert_eq!(tree.selected, 0); // clamped
    }

    #[test]
    fn visible_lines_disconnected() {
        let tree = TreePane::new(vec!["a".into(), "b".into()]);
        let lines = tree.visible_lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "  a");
        assert_eq!(lines[1].text, "  b");
    }

    #[test]
    fn expand_connection_shows_schemas_and_tables() {
        let mut tree = TreePane::new(vec!["conn".into()]);
        tree.set_connected(
            0,
            vec![
                SchemaItem {
                    name: "public".into(),
                    expanded: true,
                    tables: tables(&["users", "orders"]),
                },
                SchemaItem {
                    name: "auth".into(),
                    expanded: false,
                    tables: tables(&["accounts"]),
                },
            ],
        );

        let lines = tree.visible_lines();
        assert_eq!(lines.len(), 5); // conn, public, users, orders, auth
        assert_eq!(lines[0].text, "v conn");
        assert_eq!(lines[1].text, "  v public");
        assert_eq!(lines[2].text, "    > users");
        assert_eq!(lines[3].text, "    > orders");
        assert_eq!(lines[4].text, "  > auth");
    }

    #[test]
    fn toggle_collapses_connection() {
        let mut tree = TreePane::new(vec!["conn".into()]);
        tree.set_connected(
            0,
            vec![SchemaItem {
                name: "public".into(),
                expanded: false,
                tables: tables(&["users"]),
            }],
        );
        assert_eq!(tree.visible_lines().len(), 2); // conn + public

        tree.selected = 0;
        tree.toggle_selected();
        assert_eq!(tree.visible_lines().len(), 1); // only conn, collapsed
        assert!(matches!(
            tree.connections[0].state,
            ConnState::Connected {
                expanded: false,
                ..
            }
        ));
    }

    #[test]
    fn toggle_schema_expands_tables() {
        let mut tree = TreePane::new(vec!["conn".into()]);
        tree.set_connected(
            0,
            vec![SchemaItem {
                name: "public".into(),
                expanded: false,
                tables: tables(&["users"]),
            }],
        );
        tree.selected = 1; // public schema line
        tree.toggle_selected();
        let lines = tree.visible_lines();
        assert_eq!(lines.len(), 3); // conn, public, users
    }

    #[test]
    fn expand_table_shows_columns() {
        let mut tree = TreePane::new(vec!["conn".into()]);
        tree.set_connected(
            0,
            vec![SchemaItem {
                name: "public".into(),
                expanded: true,
                tables: tables(&["users"]),
            }],
        );
        // Table line is at index 2 (conn, public, users).
        tree.set_table_columns(
            0,
            0,
            0,
            vec![
                ColumnNode {
                    name: "id".into(),
                    type_name: "integer".into(),
                    is_pk: true,
                },
                ColumnNode {
                    name: "name".into(),
                    type_name: "text".into(),
                    is_pk: false,
                },
            ],
        );
        assert!(!tree.is_table_expanded(0, 0, 0));
        tree.set_table_expanded(0, 0, 0, true);
        assert!(tree.is_table_expanded(0, 0, 0));

        let lines = tree.visible_lines();
        // conn, public, users, <id>, <name>
        assert_eq!(lines.len(), 5);
        assert!(lines[3].text.contains("id"));
        assert!(lines[3].text.contains("PK"));
        assert!(lines[4].text.contains("name"));
        assert!(!lines[4].text.contains("PK"));
        assert!(matches!(lines[3].kind, LineKind::Column { col: 0, .. }));
    }

    #[test]
    fn expanded_table_shows_index_and_fk_detail() {
        let mut tree = TreePane::new(vec!["conn".into()]);
        tree.set_connected(
            0,
            vec![SchemaItem {
                name: "public".into(),
                expanded: true,
                tables: tables(&["users"]),
            }],
        );
        tree.set_table_columns(
            0,
            0,
            0,
            vec![ColumnNode {
                name: "id".into(),
                type_name: "int".into(),
                is_pk: true,
            }],
        );
        assert!(!tree.table_detail_loaded(0, 0, 0));
        tree.set_table_detail(
            0,
            0,
            0,
            vec!["⚿ idx_users_email (email)".into()],
            vec!["→ org_id → orgs(id)".into()],
        );
        assert!(tree.table_detail_loaded(0, 0, 0));
        tree.set_table_expanded(0, 0, 0, true);

        let text: String = tree
            .visible_lines()
            .iter()
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("idx_users_email"), "got: {text}");
        assert!(text.contains("orgs(id)"), "got: {text}");
    }

    #[test]
    fn table_accessors() {
        let mut tree = TreePane::new(vec!["conn".into()]);
        tree.set_connected(
            0,
            vec![SchemaItem {
                name: "public".into(),
                expanded: true,
                tables: tables(&["users"]),
            }],
        );
        assert_eq!(tree.schema_name(0, 0).as_deref(), Some("public"));
        assert_eq!(tree.table_name(0, 0, 0).as_deref(), Some("users"));
        assert!(!tree.table_has_columns(0, 0, 0));
    }
}
