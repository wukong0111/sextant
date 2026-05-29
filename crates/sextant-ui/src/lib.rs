use std::io::{self, Stdout, stdout};
use std::time::{Duration, Instant};

use futures::StreamExt;
use ratatui::crossterm::{
    ExecutableCommand,
    event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use sextant_core::QueryExecutor;
use tokio::sync::mpsc::UnboundedSender;

mod autocomplete;

mod editor_modal;
use editor_modal::{EditorAction, EditorModal};

mod tree_pane;
use tree_pane::{ColumnNode, ConnState, SchemaItem, TableItem, TreePane};

mod result_grid;
use result_grid::ResultGrid;

/// Messages that flow through the application event loop.
#[derive(Debug)]
pub enum AppMsg {
    /// Request to connect to a named connection.
    Connect(String),
    /// Connection succeeded.
    Connected {
        name: String,
        executor: sextant_db::SqlxExecutor,
        schemas: Vec<sextant_db::introspection::Schema>,
        metadata: std::collections::HashMap<(String, String), sextant_db::introspection::TableMeta>,
    },
    /// Connection failed.
    ConnectionFailed { name: String, error: String },
    /// Disconnect the active connection.
    Disconnect,
    /// Execute a SQL statement.
    ExecuteSql(String),
    /// Query executed successfully.
    QueryResult(sextant_core::QueryResult),
    /// Query or connection error.
    QueryError(String),
    /// Lazily-loaded index/FK detail for an expanded table.
    TableDetailLoaded {
        conn: usize,
        schema: usize,
        table: usize,
        detail: sextant_db::introspection::TableDetail,
    },
    /// Result of committing pending grid edits (rows affected, or error).
    CommitResult(Result<u64, String>),
    /// Toggle the SQL editor modal.
    ToggleEditor,
    /// A key event targeted at the editor.
    EditorKey(KeyEvent),
    /// Quit the application.
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tree,
    Grid,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Normal => write!(f, "NOR"),
            Mode::Insert => write!(f, "INS"),
        }
    }
}

/// Application state for the TUI.
pub struct App {
    pub mode: Mode,
    pub connection_name: Option<String>,
    should_quit: bool,
    tree: TreePane,
    connection_configs: Vec<sextant_core::Connection>,
    executors: std::collections::HashMap<String, sextant_db::SqlxExecutor>,
    editor_open: bool,
    editor: EditorModal,
    pending_leader: bool,
    pending_g: bool,
    /// Pending `d` of a `dd` (delete-row) chord in the grid.
    pending_d: bool,
    /// Pending grid-commit statements awaiting confirmation.
    pending_commit: Option<Vec<String>>,
    /// SQL used for the current browse, replayed to refresh after a commit.
    last_browse_sql: Option<String>,
    saved_buffers: std::collections::HashMap<String, String>,
    /// Column/PK metadata per connection, keyed by `(schema, table)`.
    table_meta: std::collections::HashMap<
        String,
        std::collections::HashMap<(String, String), sextant_db::introspection::TableMeta>,
    >,
    last_result: Option<sextant_core::QueryResult>,
    last_error: Option<String>,
    last_query_duration: Option<Duration>,
    query_start: Option<Instant>,
    focus: Focus,
    result_grid: ResultGrid,
    needs_redraw: bool,
}

impl App {
    fn new() -> io::Result<Self> {
        let connections = sextant_config::load_connections().unwrap_or_else(|e| {
            tracing::warn!("failed to load connections: {e}");
            vec![]
        });
        let names: Vec<String> = connections.iter().map(|c| c.name.clone()).collect();
        let tree = TreePane::new(names);

        Ok(Self {
            mode: Mode::Normal,
            connection_name: None,
            should_quit: false,
            tree,
            connection_configs: connections,
            executors: std::collections::HashMap::new(),
            editor_open: false,
            editor: EditorModal::new(),
            pending_leader: false,
            pending_g: false,
            pending_d: false,
            pending_commit: None,
            last_browse_sql: None,
            saved_buffers: std::collections::HashMap::new(),
            table_meta: std::collections::HashMap::new(),
            last_result: None,
            last_error: None,
            last_query_duration: None,
            query_start: None,
            focus: Focus::Tree,
            result_grid: ResultGrid::new(),
            needs_redraw: true,
        })
    }

    /// Render the application into the given frame.
    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();

        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);

        let inner = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
            .split(outer[0]);

        // Sidebar tree pane.
        self.tree.render(frame, inner[0]);

        // Main area: result grid.
        if self.result_grid.result() != &self.last_result {
            self.result_grid.set_result(self.last_result.clone());
        }
        self.result_grid.render(frame, inner[1]);

        // Editor modal (floating overlay).
        if self.editor_open {
            self.editor.render(frame, area);
        }

        // Commit-confirmation modal (floating overlay).
        if let Some(statements) = &self.pending_commit {
            render_commit_modal(frame, area, statements);
        }

        // Status line at the bottom.
        let mode_span = Span::styled(
            format!(" {} ", self.mode),
            Style::default()
                .fg(Color::Black)
                .bg(if self.mode == Mode::Normal {
                    Color::Cyan
                } else {
                    Color::Yellow
                }),
        );

        let conn_span = Span::raw(format!(
            " {} ",
            self.connection_name.as_deref().unwrap_or("no connection")
        ));

        let error_span = if let Some(ref err) = self.last_error {
            Span::styled(format!(" ERR: {} │ ", err), Style::default().fg(Color::Red))
        } else {
            Span::raw("")
        };

        let stats_span = if let Some(ref result) = self.last_result {
            let rows = result.rows.len();
            let dur = self
                .last_query_duration
                .map(|d| format!("{}ms", d.as_millis()))
                .unwrap_or_else(|| "-".into());
            Span::raw(format!(" {rows} rows / {dur} │ "))
        } else {
            Span::raw(" ")
        };

        // Editability / pending-changes indicator.
        let edit_span = if self.result_grid.result().is_some() {
            if !self.result_grid.is_editable() {
                Span::styled("🔒 │ ", Style::default().fg(Color::DarkGray))
            } else if self.result_grid.has_pending() {
                Span::styled(
                    format!("✎ {} pending │ ", self.result_grid.pending_count()),
                    Style::default().fg(Color::Yellow),
                )
            } else {
                Span::raw("")
            }
        } else {
            Span::raw("")
        };

        let hint = if self.editor_open {
            if self.mode == Mode::Normal {
                " <i> insert │ <C-e> run │ <Esc> close "
            } else {
                " <Esc> normal "
            }
        } else if self.focus == Focus::Grid && self.result_grid.is_editable() {
            " <Enter> edit │ o add │ dd del │ <C-s> commit │ <C-z> discard "
        } else {
            " <Space>e editor │ <C-q> quit "
        };
        let hint_span = Span::styled(hint, Style::default().fg(Color::DarkGray));

        let status = Line::from(vec![
            mode_span, conn_span, error_span, stats_span, edit_span, hint_span,
        ]);
        let status_bar = Paragraph::new(status).style(Style::default().bg(Color::Black));
        frame.render_widget(status_bar, outer[1]);
    }

    fn handle_key_event(&mut self, key: KeyEvent, tx: &UnboundedSender<AppMsg>) {
        if key.kind != KeyEventKind::Press {
            return;
        }

        tracing::debug!("key: {:?}, modifiers: {:?}", key.code, key.modifiers);

        if self.editor_open {
            let (new_mode, action) = self.editor.handle_key(key, self.mode);
            tracing::debug!("editor action: {:?}, new_mode: {:?}", action, new_mode);
            self.mode = new_mode;
            match action {
                EditorAction::Execute => self.run_editor_sql(tx),
                EditorAction::Save => self.save_editor_buffer(),
                EditorAction::Close => self.close_editor(),
                EditorAction::None => {}
            }
            return;
        }

        // Commit-confirmation modal swallows keys until dismissed.
        if self.pending_commit.is_some() {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') => self.confirm_commit(tx),
                KeyCode::Esc | KeyCode::Char('n') => self.pending_commit = None,
                _ => {}
            }
            return;
        }

        // Inline cell editing in the grid swallows keys until it ends.
        if self.focus == Focus::Grid && self.result_grid.is_editing() {
            if !self.result_grid.handle_edit_key(key) {
                self.mode = Mode::Normal;
            }
            return;
        }

        // `dd` chord state for row deletion (reset unless this key is `d`).
        let d_pending = self.pending_d;
        self.pending_d = false;

        match key.code {
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Tree => Focus::Grid,
                    Focus::Grid => Focus::Tree,
                };
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('j') if key.modifiers.is_empty() => {
                match self.focus {
                    Focus::Tree => self.tree.next(),
                    Focus::Grid => self.result_grid.move_down(),
                }
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('k') if key.modifiers.is_empty() => {
                match self.focus {
                    Focus::Tree => self.tree.prev(),
                    Focus::Grid => self.result_grid.move_up(),
                }
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('h') if key.modifiers.is_empty() => {
                match self.focus {
                    Focus::Tree => self.handle_tree_left(),
                    Focus::Grid => self.result_grid.move_left(),
                }
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('l') if key.modifiers.is_empty() => {
                match self.focus {
                    Focus::Tree => self.handle_tree_right(tx),
                    Focus::Grid => self.result_grid.move_right(),
                }
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('g') if key.modifiers.is_empty() => {
                if self.focus == Focus::Grid {
                    if self.pending_g {
                        self.result_grid.top();
                        self.pending_g = false;
                    } else {
                        self.pending_g = true;
                    }
                }
                self.pending_leader = false;
            }
            KeyCode::Char('G') if key.modifiers.is_empty() => {
                if self.focus == Focus::Grid {
                    self.result_grid.bottom();
                }
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Enter => {
                match self.focus {
                    Focus::Tree => self.handle_enter(tx),
                    Focus::Grid => {
                        self.result_grid.begin_edit();
                        if self.result_grid.is_editing() {
                            self.mode = Mode::Insert;
                        }
                    }
                }
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('o') if key.modifiers.is_empty() && self.focus == Focus::Grid => {
                self.result_grid.add_row();
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('d') if key.modifiers.is_empty() && self.focus == Focus::Grid => {
                if d_pending {
                    self.result_grid.mark_delete();
                } else {
                    self.pending_d = true;
                }
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('z')
                if key.modifiers.contains(KeyModifiers::CONTROL) && self.focus == Focus::Grid =>
            {
                self.result_grid.discard_changes();
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('s')
                if key.modifiers.contains(KeyModifiers::CONTROL) && self.focus == Focus::Grid =>
            {
                self.begin_commit();
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char(' ') => {
                self.pending_leader = true;
                self.pending_g = false;
            }
            KeyCode::Char('e') if self.pending_leader => {
                self.open_editor();
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('D') if key.modifiers.is_empty() && self.focus == Focus::Tree => {
                self.emit_table_ddl();
                self.pending_leader = false;
                self.pending_g = false;
            }
            _ => {
                self.pending_leader = false;
                self.pending_g = false;
            }
        }
    }

    #[cfg(test)]
    fn handle_event(&mut self, event: Event) {
        if let Event::Key(key) = event {
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            self.handle_key_event(key, &tx);
        }
    }

    fn handle_enter(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(kind) = self.tree.selected_kind() else {
            return;
        };

        match kind {
            tree_pane::LineKind::Connection { conn } => {
                let state = &self.tree.connections[conn].state;
                match state {
                    ConnState::Disconnected | ConnState::Error(_) => {
                        let name = self.tree.connections[conn].name.clone();
                        self.start_connection(&name, conn, tx);
                    }
                    ConnState::Connected { .. } | ConnState::Connecting => {}
                }
            }
            tree_pane::LineKind::Table {
                conn,
                schema,
                table,
            } => {
                self.browse_table(conn, schema, table, tx);
            }
            tree_pane::LineKind::Schema { .. }
            | tree_pane::LineKind::Column { .. }
            | tree_pane::LineKind::Detail => {
                // Schemas expand via 'l'; columns/details have no open action.
            }
        }
    }

    fn handle_tree_right(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(kind) = self.tree.selected_kind() else {
            return;
        };

        match kind {
            tree_pane::LineKind::Connection { conn } => {
                let name = self.tree.connections[conn].name.clone();
                match &self.tree.connections[conn].state {
                    ConnState::Disconnected | ConnState::Error(_) => {
                        self.start_connection(&name, conn, tx);
                    }
                    ConnState::Connected { expanded, .. } => {
                        self.connection_name = Some(name);
                        if !expanded {
                            self.tree.toggle_selected();
                        }
                    }
                    ConnState::Connecting => {}
                }
            }
            tree_pane::LineKind::Schema { conn, schema } => {
                if let Some(c) = self.tree.connections.get(conn) {
                    if let ConnState::Connected { schemas, .. } = &c.state {
                        if let Some(s) = schemas.get(schema) {
                            if !s.expanded {
                                self.tree.toggle_selected();
                            }
                        }
                    }
                }
            }
            tree_pane::LineKind::Table {
                conn,
                schema,
                table,
            } => {
                if !self.tree.is_table_expanded(conn, schema, table) {
                    self.expand_table(conn, schema, table, tx);
                }
            }
            tree_pane::LineKind::Column { .. } | tree_pane::LineKind::Detail => {}
        }
    }

    fn handle_tree_left(&mut self) {
        let Some(kind) = self.tree.selected_kind() else {
            return;
        };

        match kind {
            tree_pane::LineKind::Connection { conn } => {
                if let ConnState::Connected { expanded: true, .. } =
                    &self.tree.connections[conn].state
                {
                    self.tree.toggle_selected();
                }
            }
            tree_pane::LineKind::Schema { conn, schema } => {
                if let Some(c) = self.tree.connections.get(conn) {
                    if let ConnState::Connected { schemas, .. } = &c.state {
                        if let Some(s) = schemas.get(schema) {
                            if s.expanded {
                                self.tree.toggle_selected();
                            }
                        }
                    }
                }
            }
            tree_pane::LineKind::Table {
                conn,
                schema,
                table,
            } => {
                if self.tree.is_table_expanded(conn, schema, table) {
                    self.tree.set_table_expanded(conn, schema, table, false);
                }
            }
            tree_pane::LineKind::Column { .. } | tree_pane::LineKind::Detail => {}
        }
    }

    fn start_connection(&mut self, name: &str, conn_idx: usize, tx: &UnboundedSender<AppMsg>) {
        let Some(config) = self.connection_configs.iter().find(|c| c.name == name) else {
            return;
        };

        self.tree.set_connecting(conn_idx);
        self.connection_name = Some(format!("{name} (connecting)"));

        let password = sextant_config::connection_password(name);
        let config = config.clone();
        let tx = tx.clone();
        let name = name.to_string();

        tokio::spawn(async move {
            let mut mgr = sextant_db::ConnectionManager::new();
            match mgr.connect(&name, &config, password.as_deref()).await {
                Ok(executor) => match executor.introspect_schemas_and_tables(config.driver).await {
                    Ok(schemas) => {
                        let mut metadata = std::collections::HashMap::new();
                        for schema in &schemas {
                            match executor
                                .introspect_columns(config.driver, &schema.name)
                                .await
                            {
                                Ok(tables) => {
                                    for (table, meta) in tables {
                                        metadata.insert((schema.name.clone(), table), meta);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "failed to introspect columns for {}: {e}",
                                        schema.name
                                    );
                                }
                            }
                        }
                        let _ = tx.send(AppMsg::Connected {
                            name,
                            executor,
                            schemas,
                            metadata,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::ConnectionFailed {
                            name,
                            error: format!("{e}"),
                        });
                    }
                },
                Err(e) => {
                    let _ = tx.send(AppMsg::ConnectionFailed {
                        name,
                        error: format!("{e}"),
                    });
                }
            }
        });
    }

    fn handle_msg(&mut self, msg: AppMsg, tx: &UnboundedSender<AppMsg>) {
        match msg {
            AppMsg::Connected {
                name,
                executor,
                schemas,
                metadata,
            } => {
                let schema_items = schemas
                    .into_iter()
                    .map(|s| SchemaItem {
                        name: s.name,
                        expanded: true,
                        tables: s.tables.into_iter().map(TableItem::new).collect(),
                    })
                    .collect();

                if let Some(idx) = self.tree.connection_index_by_name(&name) {
                    self.tree.set_connected(idx, schema_items);
                }
                self.executors.insert(name.clone(), executor);
                self.table_meta.insert(name.clone(), metadata);
                self.connection_name = Some(name);
            }
            AppMsg::ConnectionFailed { name, error } => {
                if let Some(idx) = self.tree.connection_index_by_name(&name) {
                    self.tree.set_error(idx, error.clone());
                }
                self.connection_name = Some(format!("{name}: {error}"));
            }
            AppMsg::QueryResult(result) => {
                let rows = result.rows.len();
                self.last_result = Some(result);
                self.last_error = None;
                self.last_query_duration = self.query_start.take().map(|t| t.elapsed());
                tracing::info!("query returned {} rows", rows);
            }
            AppMsg::QueryError(error) => {
                tracing::warn!("query error: {}", error);
                self.last_error = Some(error);
            }
            AppMsg::TableDetailLoaded {
                conn,
                schema,
                table,
                detail,
            } => {
                let indexes = detail
                    .indexes
                    .iter()
                    .map(|i| {
                        let unique = if i.unique { " UNIQUE" } else { "" };
                        format!("⚿ {} ({}){}", i.name, i.columns.join(", "), unique)
                    })
                    .collect();
                let foreign_keys = detail
                    .foreign_keys
                    .iter()
                    .map(|f| {
                        format!(
                            "→ {} → {}({})",
                            f.columns.join(", "),
                            f.ref_table,
                            f.ref_columns.join(", ")
                        )
                    })
                    .collect();
                self.tree
                    .set_table_detail(conn, schema, table, indexes, foreign_keys);
            }
            AppMsg::CommitResult(Ok(affected)) => {
                tracing::info!("commit affected {affected} rows");
                self.result_grid.discard_changes();
                self.last_error = None;
                // Replay the browse query so the grid reflects committed state.
                if let (Some(name), Some(sql)) =
                    (self.connection_name.clone(), self.last_browse_sql.clone())
                {
                    self.run_sql(&name, sql, tx);
                }
            }
            AppMsg::CommitResult(Err(error)) => {
                tracing::warn!("commit error: {}", error);
                // Keep pending edits so the user can fix and retry.
                self.last_error = Some(error);
            }
            AppMsg::Quit => {
                self.should_quit = true;
            }
            _ => {}
        }
    }

    fn open_editor(&mut self) {
        self.editor_open = true;
        self.mode = Mode::Normal;
        if let Some(name) = self.connection_name.clone() {
            if let Some(buf) = self.saved_buffers.get(&name).cloned() {
                self.editor.set_content(&buf);
            }
            let index = self.build_completion_index(&name);
            self.editor.set_completion_source(index);
        }
    }

    /// Build the autocomplete table/column index for a connection.
    fn build_completion_index(&self, conn_name: &str) -> autocomplete::SchemaIndex {
        let mut index = autocomplete::SchemaIndex::new();
        if let Some(tables) = self.table_meta.get(conn_name) {
            for ((_schema, table), meta) in tables {
                let columns = meta.columns.iter().map(|c| c.name.clone()).collect();
                index.add_table(table.clone(), columns);
            }
        }
        index
    }

    fn close_editor(&mut self) {
        self.editor_open = false;
        self.mode = Mode::Normal;
    }

    fn save_editor_buffer(&mut self) {
        if let Some(name) = &self.connection_name {
            self.saved_buffers
                .insert(name.clone(), self.editor.content());
            self.editor.mark_saved();
        }
    }

    fn run_editor_sql(&mut self, tx: &UnboundedSender<AppMsg>) {
        let sql = self.editor.content();
        let Some(name) = self.connection_name.clone() else {
            tracing::warn!("run_editor_sql: no connection_name");
            return;
        };
        // Ad-hoc editor results are not tied to a browseable table → read-only.
        self.result_grid.set_edit_context(None);
        self.last_browse_sql = None;
        self.run_sql(&name, sql, tx);
    }

    /// Spawn a query against the named connection's executor; the result is
    /// delivered back through the channel as `QueryResult`/`QueryError`.
    fn run_sql(&mut self, conn_name: &str, sql: String, tx: &UnboundedSender<AppMsg>) {
        tracing::info!("run_sql: conn='{}', sql='{}'", conn_name, sql.trim());
        let Some(executor) = self.executors.get(conn_name).cloned() else {
            tracing::warn!(
                "run_sql: no executor for '{}'. available: {:?}",
                conn_name,
                self.executors.keys().collect::<Vec<_>>()
            );
            return;
        };
        self.query_start = Some(Instant::now());
        let tx = tx.clone();
        tokio::spawn(async move {
            match executor.execute(&sql).await {
                Ok(result) => {
                    let _ = tx.send(AppMsg::QueryResult(result));
                }
                Err(e) => {
                    let _ = tx.send(AppMsg::QueryError(format!("{e}")));
                }
            }
        });
    }

    /// Driver for a connection by name (from the loaded config).
    fn driver_for(&self, conn_name: &str) -> Option<sextant_core::Driver> {
        self.connection_configs
            .iter()
            .find(|c| c.name == conn_name)
            .map(|c| c.driver)
    }

    /// Expand a table node: fill column children from cached metadata (sync)
    /// and lazily load index/FK detail (async, once).
    fn expand_table(
        &mut self,
        conn: usize,
        schema: usize,
        table: usize,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let conn_name = self.tree.connections[conn].name.clone();
        if !self.tree.table_has_columns(conn, schema, table) {
            if let (Some(schema_name), Some(table_name)) = (
                self.tree.schema_name(conn, schema),
                self.tree.table_name(conn, schema, table),
            ) {
                if let Some(meta) = self
                    .table_meta
                    .get(&conn_name)
                    .and_then(|m| m.get(&(schema_name, table_name)))
                {
                    let columns = meta
                        .columns
                        .iter()
                        .map(|c| ColumnNode {
                            name: c.name.clone(),
                            type_name: c.type_name.clone(),
                            is_pk: c.is_primary_key,
                        })
                        .collect();
                    self.tree.set_table_columns(conn, schema, table, columns);
                }
            }
        }
        self.tree.set_table_expanded(conn, schema, table, true);

        // Lazily load indexes/FKs the first time the table is expanded.
        if !self.tree.table_detail_loaded(conn, schema, table) {
            if let (Some(schema_name), Some(table_name), Some(driver), Some(executor)) = (
                self.tree.schema_name(conn, schema),
                self.tree.table_name(conn, schema, table),
                self.driver_for(&conn_name),
                self.executors.get(&conn_name).cloned(),
            ) {
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Ok(detail) = executor
                        .introspect_table_detail(driver, &schema_name, &table_name)
                        .await
                    {
                        let _ = tx.send(AppMsg::TableDetailLoaded {
                            conn,
                            schema,
                            table,
                            detail,
                        });
                    }
                });
            }
        }
    }

    /// Browse a table's rows: run `SELECT * FROM <table> LIMIT 500` and show
    /// the result in the grid, switching focus to it.
    fn browse_table(
        &mut self,
        conn: usize,
        schema: usize,
        table: usize,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let conn_name = self.tree.connections[conn].name.clone();
        let (Some(schema_name), Some(table_name), Some(driver)) = (
            self.tree.schema_name(conn, schema),
            self.tree.table_name(conn, schema, table),
            self.driver_for(&conn_name),
        ) else {
            return;
        };

        let sql = format!(
            "SELECT * FROM {} LIMIT 500",
            sextant_db::qualified_table(driver, &schema_name, &table_name)
        );

        // Make the grid editable for this table (read-only if it has no PK).
        let pk_columns = self
            .table_meta
            .get(&conn_name)
            .and_then(|m| m.get(&(schema_name.clone(), table_name.clone())))
            .map(|meta| meta.primary_key.clone())
            .unwrap_or_default();
        self.result_grid
            .set_edit_context(Some(result_grid::EditContext {
                driver,
                schema: schema_name,
                table: table_name,
                pk_columns,
            }));

        self.connection_name = Some(conn_name.clone());
        self.last_browse_sql = Some(sql.clone());
        self.run_sql(&conn_name, sql, tx);
        self.focus = Focus::Grid;
    }

    /// Build the commit-confirmation modal from the grid's pending changes.
    fn begin_commit(&mut self) {
        if !self.result_grid.is_editable() || !self.result_grid.has_pending() {
            return;
        }
        let statements = self.result_grid.build_commit_statements();
        if !statements.is_empty() {
            self.pending_commit = Some(statements);
        }
    }

    /// Apply the confirmed statements in a transaction on the active connection.
    fn confirm_commit(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(statements) = self.pending_commit.take() else {
            return;
        };
        let Some(name) = self.connection_name.clone() else {
            return;
        };
        let Some(executor) = self.executors.get(&name).cloned() else {
            return;
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            let result = executor
                .execute_transaction(&statements)
                .await
                .map_err(|e| format!("{e}"));
            let _ = tx.send(AppMsg::CommitResult(result));
        });
    }

    /// Emit a `CREATE TABLE` skeleton for the selected table into the editor.
    fn emit_table_ddl(&mut self) {
        let Some(tree_pane::LineKind::Table {
            conn,
            schema,
            table,
        }) = self.tree.selected_kind()
        else {
            return;
        };
        let conn_name = self.tree.connections[conn].name.clone();
        let (Some(schema_name), Some(table_name), Some(driver)) = (
            self.tree.schema_name(conn, schema),
            self.tree.table_name(conn, schema, table),
            self.driver_for(&conn_name),
        ) else {
            return;
        };
        let Some(meta) = self
            .table_meta
            .get(&conn_name)
            .and_then(|m| m.get(&(schema_name.clone(), table_name.clone())))
        else {
            return;
        };
        let ddl = sextant_db::generate_create_table(driver, &schema_name, &table_name, meta);

        self.connection_name = Some(conn_name);
        self.open_editor();
        let existing = self.editor.content();
        let combined = if existing.trim().is_empty() {
            ddl
        } else {
            format!("{existing}\n\n{ddl}")
        };
        self.editor.set_content(&combined);
    }
}

/// Run the TUI event loop until the user quits.
pub fn run() -> io::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async())
}

async fn run_async() -> io::Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = App::new()?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AppMsg>();
    let mut event_stream = crossterm::event::EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));

    loop {
        if app.needs_redraw {
            terminal.draw(|frame| app.render(frame))?;
            app.needs_redraw = false;
        }

        tokio::select! {
            biased;
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        app.handle_key_event(key, &tx);
                        app.needs_redraw = true;
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        app.needs_redraw = true;
                    }
                    Some(Ok(_)) => {
                        // Ignore FocusGained, FocusLost, Mouse, Paste, etc.
                    }
                    Some(Err(_)) => break,
                    None => break,
                }
            }
            Some(msg) = rx.recv() => {
                app.handle_msg(msg, &tx);
                app.needs_redraw = true;
            }
            _ = tick.tick() => {
                if app.editor_open {
                    app.needs_redraw = true;
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    restore_terminal(&mut terminal)?;
    Ok(())
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    Terminal::new(backend)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Render the commit-confirmation modal listing the statements to be run.
fn render_commit_modal(frame: &mut Frame, area: Rect, statements: &[String]) {
    let width = (area.width as f32 * 0.7) as u16;
    let height = (statements.len() as u16 + 4).min(area.height.max(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        "Commit these changes?",
        Style::default().fg(Color::Yellow),
    ))];
    for s in statements {
        lines.push(Line::from(Span::raw(format!("  {s}"))));
    }
    lines.push(Line::from(Span::styled(
        "<Enter>/y confirm   <Esc>/n cancel",
        Style::default().fg(Color::DarkGray),
    )));

    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" Confirm commit ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .style(Style::default().bg(Color::Black)),
        ),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn test_app() -> App {
        App::new().unwrap()
    }

    #[test]
    fn app_default_state() {
        let app = test_app();
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.connection_name, None);
        assert!(!app.should_quit);
    }

    #[test]
    fn renders_status_line() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = test_app();

        terminal.draw(|frame| app.render(frame)).unwrap();

        let buf = terminal.backend().buffer();
        let last_row = buf.content.chunks(buf.area.width as usize).last().unwrap();
        let text: String = last_row.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("NOR"), "status line should show mode: {text}");
        assert!(
            text.contains("no connection"),
            "status line should show connection: {text}"
        );
    }

    #[test]
    fn renders_insert_mode_in_yellow() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = test_app();
        app.mode = Mode::Insert;

        terminal.draw(|frame| app.render(frame)).unwrap();

        let buf = terminal.backend().buffer();
        let last_row = buf.content.chunks(buf.area.width as usize).last().unwrap();
        let text: String = last_row.iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("INS"),
            "status line should show Insert mode: {text}"
        );

        let idx = last_row.iter().position(|c| c.symbol() == "I").unwrap();
        assert_eq!(last_row[idx].style().bg, Some(Color::Yellow));
    }

    #[test]
    fn renders_connection_name_when_set() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = test_app();
        app.connection_name = Some("local-pg".into());

        terminal.draw(|frame| app.render(frame)).unwrap();

        let buf = terminal.backend().buffer();
        let last_row = buf.content.chunks(buf.area.width as usize).last().unwrap();
        let text: String = last_row.iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("local-pg"),
            "status line should show connection name: {text}"
        );
        assert!(
            !text.contains("no connection"),
            "status line should not show fallback: {text}"
        );
    }

    #[test]
    fn main_area_has_black_background() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = test_app();

        terminal.draw(|frame| app.render(frame)).unwrap();

        let buf = terminal.backend().buffer();
        // Main area is the right 75% of rows 0..8 (not sidebar, not status line).
        // With width 40, sidebar is 25% = 10 cols, main starts at col 10.
        for y in [0, 5] {
            let cell = &buf[(15, y)];
            assert_eq!(
                cell.style().bg,
                Some(Color::Black),
                "bg at (15,{y}) should be Black"
            );
        }
    }

    #[test]
    fn layout_leaves_last_row_for_status() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = test_app();

        terminal.draw(|frame| app.render(frame)).unwrap();

        let buf = terminal.backend().buffer();
        let rows: Vec<String> = buf
            .content
            .chunks(buf.area.width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect())
            .collect();

        // Row 9 should contain the status line text.
        assert!(
            rows[9].contains("NOR"),
            "last row should contain status: {}",
            rows[9]
        );
    }

    #[test]
    fn sidebar_is_rendered() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = test_app();

        terminal.draw(|frame| app.render(frame)).unwrap();

        let buf = terminal.backend().buffer();
        // Sidebar occupies cols 0..9 (25% of 40). It should have a right border.
        // Check that column 9 has a border character.
        let border_cell = &buf[(9, 0)];
        assert!(
            border_cell.symbol() == "│"
                || border_cell.symbol() == "┐"
                || border_cell.symbol() == "┘"
                || border_cell.symbol() == "┤",
            "expected border at col 9, got: {}",
            border_cell.symbol()
        );
    }

    #[test]
    fn ctrl_q_sets_should_quit() {
        let mut app = test_app();
        assert!(!app.should_quit);

        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
        )));

        assert!(app.should_quit);
    }

    #[test]
    fn ignores_key_release() {
        let mut app = test_app();
        app.handle_event(Event::Key(
            ratatui::crossterm::event::KeyEvent::new_with_kind(
                KeyCode::Char('q'),
                KeyModifiers::CONTROL,
                KeyEventKind::Release,
            ),
        ));
        assert!(!app.should_quit);
    }

    #[test]
    fn ignores_plain_q() {
        let mut app = test_app();
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
        )));
        assert!(!app.should_quit);
    }

    #[test]
    fn ignores_resize_events() {
        let mut app = test_app();
        app.handle_event(Event::Resize(80, 24));
        assert!(!app.should_quit);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.connection_name, None);
    }

    #[test]
    fn j_moves_selection_down() {
        let mut app = test_app();
        // By default, the app loads connections from the user's config.
        // If there are no connections, this test is a no-op but still valid.
        let initial = app.tree.selected;
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        )));
        // Selection should either stay the same (if only one item) or increase.
        assert!(app.tree.selected >= initial);
    }

    #[test]
    fn k_moves_selection_up() {
        let mut app = test_app();
        // Move down first if possible, then up.
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        )));
        let after_j = app.tree.selected;
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE,
        )));
        assert!(app.tree.selected <= after_j);
    }

    #[test]
    fn editor_toggle_with_space_e() {
        let mut app = test_app();
        assert!(!app.editor_open);
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char(' '),
            KeyModifiers::NONE,
        )));
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('e'),
            KeyModifiers::NONE,
        )));
        assert!(app.editor_open);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn editor_close_with_esc_in_normal() {
        let mut app = test_app();
        app.open_editor();
        assert!(app.editor_open);
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert!(!app.editor_open);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn editor_mode_switch_i_esc() {
        let mut app = test_app();
        app.open_editor();
        assert_eq!(app.mode, Mode::Normal);
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('i'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.mode, Mode::Insert);
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn editor_renders_when_open() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = test_app();
        app.open_editor();

        terminal.draw(|frame| app.render(frame)).unwrap();

        let buf = terminal.backend().buffer();
        // The modal should contain the "SQL Editor" title somewhere.
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("SQL Editor"),
            "modal should render title: {text}"
        );
    }

    #[test]
    fn editor_saves_buffer_in_memory() {
        let mut app = test_app();
        app.connection_name = Some("test-conn".into());
        app.open_editor();

        // Type something in insert mode.
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('i'),
            KeyModifiers::NONE,
        )));
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('S'),
            KeyModifiers::NONE,
        )));
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('E'),
            KeyModifiers::NONE,
        )));
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('L'),
            KeyModifiers::NONE,
        )));

        // Return to normal mode, then save buffer with Ctrl+S.
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('s'),
            KeyModifiers::CONTROL,
        )));

        // Close modal.
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert!(!app.editor_open);

        // Clear content to prove restoration works.
        app.editor.set_content("");
        app.open_editor();
        assert_eq!(app.editor.content(), "SEL");
    }

    #[test]
    fn tab_cycles_focus() {
        let mut app = test_app();
        assert_eq!(app.focus, Focus::Tree);
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.focus, Focus::Grid);
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.focus, Focus::Tree);
    }

    #[test]
    fn grid_renders_when_result_present() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = test_app();
        app.last_result = Some(sextant_core::QueryResult {
            columns: vec![sextant_core::Column {
                name: "id".into(),
                type_name: "int".into(),
            }],
            rows: vec![vec![sextant_core::CellValue::I64(42)]],
            rows_affected: None,
        });

        terminal.draw(|frame| app.render(frame)).unwrap();

        let buf = terminal.backend().buffer();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("id"), "grid should render header: {text}");
        assert!(text.contains("42"), "grid should render cell value: {text}");
    }

    #[test]
    fn status_line_shows_row_count_and_duration() {
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = test_app();
        app.connection_name = Some("local-pg".into());
        app.last_result = Some(sextant_core::QueryResult {
            columns: vec![sextant_core::Column {
                name: "x".into(),
                type_name: "int".into(),
            }],
            rows: vec![vec![sextant_core::CellValue::I64(1)]],
            rows_affected: None,
        });
        app.last_query_duration = Some(Duration::from_millis(38));

        terminal.draw(|frame| app.render(frame)).unwrap();

        let buf = terminal.backend().buffer();
        let last_row = buf.content.chunks(buf.area.width as usize).last().unwrap();
        let text: String = last_row.iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("1 rows / 38ms"),
            "status line should show rows and duration: {text}"
        );
    }

    #[test]
    fn gg_moves_grid_to_top() {
        let mut app = test_app();
        app.focus = Focus::Grid;
        app.last_result = Some(sextant_core::QueryResult {
            columns: vec![sextant_core::Column {
                name: "x".into(),
                type_name: "int".into(),
            }],
            rows: vec![
                vec![sextant_core::CellValue::I64(1)],
                vec![sextant_core::CellValue::I64(2)],
            ],
            rows_affected: None,
        });
        app.result_grid.set_result(app.last_result.clone());
        app.result_grid.bottom();
        assert_eq!(app.result_grid.cursor_row(), 1);

        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('g'),
            KeyModifiers::NONE,
        )));
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('g'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.result_grid.cursor_row(), 0);
    }

    #[test]
    fn g_moves_grid_to_bottom() {
        let mut app = test_app();
        app.focus = Focus::Grid;
        app.last_result = Some(sextant_core::QueryResult {
            columns: vec![sextant_core::Column {
                name: "x".into(),
                type_name: "int".into(),
            }],
            rows: vec![
                vec![sextant_core::CellValue::I64(1)],
                vec![sextant_core::CellValue::I64(2)],
            ],
            rows_affected: None,
        });
        app.result_grid.set_result(app.last_result.clone());
        assert_eq!(app.result_grid.cursor_row(), 0);

        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('G'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.result_grid.cursor_row(), 1);
    }

    #[test]
    fn cursor_persists_after_render() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = test_app();
        app.focus = Focus::Grid;
        app.last_result = Some(sextant_core::QueryResult {
            columns: vec![sextant_core::Column {
                name: "x".into(),
                type_name: "int".into(),
            }],
            rows: vec![
                vec![sextant_core::CellValue::I64(1)],
                vec![sextant_core::CellValue::I64(2)],
            ],
            rows_affected: None,
        });

        // Initial render sets the result.
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert_eq!(app.result_grid.cursor_row(), 0);

        // Move down.
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.result_grid.cursor_row(), 1);

        // Render again — cursor must NOT reset to 0.
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert_eq!(app.result_grid.cursor_row(), 1);
    }

    #[test]
    fn grid_shows_active_cell_highlighted() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = test_app();
        app.focus = Focus::Grid;
        app.last_result = Some(sextant_core::QueryResult {
            columns: vec![
                sextant_core::Column {
                    name: "id".into(),
                    type_name: "int".into(),
                },
                sextant_core::Column {
                    name: "name".into(),
                    type_name: "text".into(),
                },
            ],
            rows: vec![
                vec![
                    sextant_core::CellValue::I64(1),
                    sextant_core::CellValue::String("Alice".into()),
                ],
                vec![
                    sextant_core::CellValue::I64(2),
                    sextant_core::CellValue::String("Bob".into()),
                ],
            ],
            rows_affected: None,
        });

        terminal.draw(|frame| app.render(frame)).unwrap();

        let buf = terminal.backend().buffer();
        // Find the cell that contains "1" (first row, first col = active cell).
        // With our layout, the grid starts at col 10 (sidebar is 25% of 40 = 10 cols).
        // Row 0 is the first data row after the header.
        // Header is row 0, first data row is row 1.
        // Let's find the position of "1" in the buffer.
        let mut found_one = false;
        // The grid starts after the sidebar (25% of width = 10 cols for 40-wide).
        let sidebar_width = (buf.area.width as f32 * 0.25) as u16;
        for y in 0..buf.area.height {
            for x in sidebar_width..buf.area.width {
                if buf[(x, y)].symbol() == "1" {
                    let style = buf[(x, y)].style();
                    assert_eq!(
                        style.bg,
                        Some(Color::Yellow),
                        "active cell (0,0) should have Yellow bg, got {:?} at ({},{})",
                        style.bg,
                        x,
                        y
                    );
                    found_one = true;
                }
            }
        }
        assert!(found_one, "should find cell value '1' in rendered grid");
    }

    /// Build an App whose tree has one connected SQLite connection `c` with a
    /// `main.users(id PK, name)` table cached in `table_meta`.
    fn app_with_users_table() -> App {
        let mut app = test_app();
        app.tree = TreePane::new(vec!["c".into()]);
        app.connection_configs = vec![sextant_core::Connection {
            name: "c".into(),
            driver: sextant_core::Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl_mode: None,
            path: Some(":memory:".into()),
            keyring_key: None,
        }];
        app.tree.set_connected(
            0,
            vec![SchemaItem {
                name: "main".into(),
                expanded: true,
                tables: vec![TableItem::new("users".into())],
            }],
        );
        let mut meta = std::collections::HashMap::new();
        meta.insert(
            ("main".to_string(), "users".to_string()),
            sextant_db::introspection::TableMeta {
                columns: vec![
                    sextant_db::introspection::ColumnMeta {
                        name: "id".into(),
                        type_name: "INTEGER".into(),
                        nullable: false,
                        default: None,
                        is_primary_key: true,
                    },
                    sextant_db::introspection::ColumnMeta {
                        name: "name".into(),
                        type_name: "TEXT".into(),
                        nullable: true,
                        default: None,
                        is_primary_key: false,
                    },
                ],
                primary_key: vec!["id".into()],
            },
        );
        app.table_meta.insert("c".into(), meta);
        app
    }

    #[test]
    fn expand_table_fills_columns_from_metadata() {
        let mut app = app_with_users_table();
        assert!(!app.tree.is_table_expanded(0, 0, 0));

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.expand_table(0, 0, 0, &tx);

        assert!(app.tree.is_table_expanded(0, 0, 0));
        assert!(app.tree.table_has_columns(0, 0, 0));
    }

    #[test]
    fn d_emits_create_table_ddl_into_editor() {
        let mut app = app_with_users_table();
        // Lines: connection(0), schema(1), table(2).
        app.tree.selected = 2;

        app.emit_table_ddl();

        assert!(app.editor_open);
        let content = app.editor.content();
        assert!(content.contains("CREATE TABLE"), "got: {content}");
        assert!(content.contains("\"users\""), "got: {content}");
        assert!(content.contains("PRIMARY KEY (\"id\")"), "got: {content}");
    }

    #[test]
    fn ctrl_s_opens_commit_modal_and_esc_cancels() {
        let mut app = test_app();
        app.focus = Focus::Grid;
        app.result_grid.set_result(Some(sextant_core::QueryResult {
            columns: vec![
                sextant_core::Column {
                    name: "id".into(),
                    type_name: "int".into(),
                },
                sextant_core::Column {
                    name: "name".into(),
                    type_name: "text".into(),
                },
            ],
            rows: vec![vec![
                sextant_core::CellValue::I64(1),
                sextant_core::CellValue::String("Alice".into()),
            ]],
            rows_affected: None,
        }));
        app.result_grid
            .set_edit_context(Some(result_grid::EditContext {
                driver: sextant_core::Driver::Sqlite,
                schema: "main".into(),
                table: "users".into(),
                pk_columns: vec!["id".into()],
            }));
        app.result_grid.mark_delete();
        assert!(app.result_grid.has_pending());

        // Ctrl+S opens the confirmation modal with the generated statements.
        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Char('s'),
            KeyModifiers::CONTROL,
        )));
        let pending = app.pending_commit.as_ref().expect("modal should open");
        assert_eq!(pending.len(), 1);
        assert!(pending[0].starts_with("DELETE FROM"));

        // Esc cancels the commit.
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Esc)));
        assert!(app.pending_commit.is_none());
    }
}
