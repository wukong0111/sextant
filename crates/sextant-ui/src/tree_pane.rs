//! Sidebar tree pane for connections, schemas and tables.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};

/// A schema with its tables inside the tree.
#[derive(Debug, Clone)]
pub struct SchemaItem {
    pub name: String,
    pub expanded: bool,
    pub tables: Vec<String>,
}

/// Connection state within the tree.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected { expanded: bool, schemas: Vec<SchemaItem> },
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
    Connection,
    Schema { conn: usize, schema: usize },
    Table { conn: usize, schema: usize, table: usize },
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

    /// Toggle expand/collapse on the selected connection or schema.
    pub fn toggle_selected(&mut self) {
        let lines = self.visible_lines();
        let Some(line) = lines.get(self.selected) else { return };
        match line.kind {
            LineKind::Connection => {
                // Count connections up to this point to find index.
                let mut conn_idx = 0;
                let mut line_idx = 0;
                for (i, c) in self.connections.iter().enumerate() {
                    if line_idx == self.selected {
                        conn_idx = i;
                        break;
                    }
                    line_idx += 1;
                    if let ConnState::Connected { expanded: true, schemas } = &c.state {
                        for s in schemas {
                            line_idx += 1;
                            if s.expanded {
                                line_idx += s.tables.len();
                            }
                        }
                    }
                }
                if let ConnState::Connected { expanded, .. } =
                    &mut self.connections[conn_idx].state
                {
                    *expanded = !*expanded;
                }
            }
            LineKind::Schema { conn, schema } => {
                if let Some(c) = self.connections.get_mut(conn) {
                    if let ConnState::Connected { schemas, .. } = &mut c.state {
                        if let Some(s) = schemas.get_mut(schema) {
                            s.expanded = !s.expanded;
                        }
                    }
                }
            }
            LineKind::Table { .. } => {}
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

    /// Return the connection index if the current selection is a connection line.
    pub fn selected_connection_index(&self) -> Option<usize> {
        let lines = self.visible_lines();
        let line = lines.get(self.selected)?;
        if !matches!(line.kind, LineKind::Connection) {
            return None;
        }
        let mut conn_idx = 0;
        let mut line_idx = 0;
        for (i, c) in self.connections.iter().enumerate() {
            if line_idx == self.selected {
                conn_idx = i;
                break;
            }
            line_idx += 1;
            if let ConnState::Connected { expanded: true, schemas } = &c.state {
                for s in schemas {
                    line_idx += 1;
                    if s.expanded {
                        line_idx += s.tables.len();
                    }
                }
            }
        }
        Some(conn_idx)
    }

    /// Find the index of a connection by name.
    pub fn connection_index_by_name(&self, name: &str) -> Option<usize> {
        self.connections.iter().position(|c| c.name == name)
    }

    fn visible_lines(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        for (ci, conn) in self.connections.iter().enumerate() {
            let prefix = match &conn.state {
                ConnState::Disconnected => "  ",
                ConnState::Connecting => "◌ ",
                ConnState::Connected { expanded: false, .. } => "> ",
                ConnState::Connected { expanded: true, .. } => "v ",
                ConnState::Error(_) => "! ",
            };
            lines.push(Line {
                kind: LineKind::Connection,
                text: format!("{}{}", prefix, conn.name),
            });

            if let ConnState::Connected { expanded: true, schemas } = &conn.state {
                for (si, schema) in schemas.iter().enumerate() {
                    lines.push(Line {
                        kind: LineKind::Schema {
                            conn: ci,
                            schema: si,
                        },
                        text: format!("  {}", schema.name),
                    });
                    if schema.expanded {
                        for (ti, table) in schema.tables.iter().enumerate() {
                            lines.push(Line {
                                kind: LineKind::Table {
                                    conn: ci,
                                    schema: si,
                                    table: ti,
                                },
                                text: format!("    {}", table),
                            });
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
                    tables: vec!["users".into(), "orders".into()],
                },
                SchemaItem {
                    name: "auth".into(),
                    expanded: false,
                    tables: vec!["accounts".into()],
                },
            ],
        );

        let lines = tree.visible_lines();
        assert_eq!(lines.len(), 5); // conn, public, users, orders, auth
        assert_eq!(lines[0].text, "v conn");
        assert_eq!(lines[1].text, "  public");
        assert_eq!(lines[2].text, "    users");
        assert_eq!(lines[3].text, "    orders");
        assert_eq!(lines[4].text, "  auth");
    }

    #[test]
    fn toggle_collapses_connection() {
        let mut tree = TreePane::new(vec!["conn".into()]);
        tree.set_connected(
            0,
            vec![SchemaItem {
                name: "public".into(),
                expanded: false,
                tables: vec!["users".into()],
            }],
        );
        assert_eq!(tree.visible_lines().len(), 2); // conn + public

        tree.selected = 0;
        tree.toggle_selected();
        assert_eq!(tree.visible_lines().len(), 1); // only conn, collapsed
        assert!(matches!(
            tree.connections[0].state,
            ConnState::Connected { expanded: false, .. }
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
                tables: vec!["users".into()],
            }],
        );
        tree.selected = 1; // public schema line
        tree.toggle_selected();
        let lines = tree.visible_lines();
        assert_eq!(lines.len(), 3); // conn, public, users
    }

    #[test]
    fn selected_connection_index() {
        let mut tree = TreePane::new(vec!["a".into(), "b".into()]);
        assert_eq!(tree.selected_connection_index(), Some(0));
        tree.selected = 1;
        assert_eq!(tree.selected_connection_index(), Some(1));
    }
}
