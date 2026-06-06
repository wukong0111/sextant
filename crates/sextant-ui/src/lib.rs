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
    /// Query history loaded from the state store (for the history picker).
    HistoryLoaded(Vec<sextant_state::HistoryEntry>),
    /// Recent files loaded from the state store (for the recent-files picker).
    RecentFilesLoaded(Vec<sextant_state::FileEntry>),
    /// An export finished: the written path, or an error message.
    ExportFinished(Result<std::path::PathBuf, String>),
    /// An import finished: rows affected, or an error message.
    ImportFinished(Result<u64, String>),
    /// Quit the application.
    Quit,
}

/// A modal list picker (query history or recent files).
struct Picker {
    title: String,
    items: Vec<PickerItem>,
    selected: usize,
}

/// A single selectable row in a [`Picker`].
struct PickerItem {
    label: String,
    action: PickerAction,
}

/// What selecting a [`PickerItem`] does.
enum PickerAction {
    /// Load SQL text into the editor.
    LoadSql(String),
    /// Read a `.sql` file from disk and load it into the editor.
    OpenFile(std::path::PathBuf),
    /// Export the current result set in the chosen format.
    Export(sextant_db::ExportFormat),
}

/// An import in progress: the chosen target table and the file path being typed.
struct ImportPrompt {
    conn: String,
    schema: String,
    table: String,
    driver: sextant_core::Driver,
    path: String,
}

/// A destructive editor statement awaiting confirmation before it runs.
struct DangerousStmt {
    /// Connection whose executor will run the statement.
    conn: String,
    /// The SQL to run on confirmation.
    sql: String,
    /// Why it was flagged (shown in the modal).
    reason: &'static str,
}

/// A parsed import awaiting confirmation in a modal.
struct PendingImport {
    /// Connection whose executor runs the statements.
    conn: String,
    /// Human-readable summary lines for the confirm modal.
    summary: Vec<String>,
    /// Statements to run in a single transaction.
    statements: Vec<String>,
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
    /// Filename being entered to save the active buffer (Save-as prompt).
    save_prompt: Option<String>,
    /// Whether the quit-with-unsaved-buffers prompt is showing.
    quit_prompt: bool,
    /// Local state store (query history + recent files); `None` if unavailable.
    state_store: Option<sextant_state::StateStore>,
    /// Active modal list picker (history / recent files), if any.
    picker: Option<Picker>,
    /// Active import file-path prompt, if any.
    import_prompt: Option<ImportPrompt>,
    /// Parsed import awaiting confirmation, if any.
    pending_import: Option<PendingImport>,
    /// Destructive editor statement awaiting confirmation, if any.
    pending_dangerous: Option<DangerousStmt>,
    saved_buffers: std::collections::HashMap<String, String>,
    /// Column/PK metadata per connection, keyed by `(schema, table)`.
    table_meta: std::collections::HashMap<
        String,
        std::collections::HashMap<(String, String), sextant_db::introspection::TableMeta>,
    >,
    last_result: Option<sextant_core::QueryResult>,
    last_error: Option<String>,
    /// Transient success notice shown in the status line (e.g. export path).
    last_notice: Option<String>,
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
            save_prompt: None,
            quit_prompt: false,
            state_store: None,
            picker: None,
            import_prompt: None,
            pending_import: None,
            pending_dangerous: None,
            saved_buffers: std::collections::HashMap::new(),
            table_meta: std::collections::HashMap::new(),
            last_result: None,
            last_error: None,
            last_notice: None,
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

        // Save-as filename prompt.
        if let Some(name) = &self.save_prompt {
            render_save_prompt(frame, area, name);
        }

        // Quit-with-unsaved-buffers prompt.
        if self.quit_prompt {
            render_quit_prompt(frame, area);
        }

        // Modal list picker (history / recent files).
        if let Some(picker) = &self.picker {
            render_picker(frame, area, picker);
        }

        // Import file-path prompt.
        if let Some(prompt) = &self.import_prompt {
            render_import_prompt(frame, area, prompt);
        }

        // Import confirmation modal.
        if let Some(pending) = &self.pending_import {
            render_import_modal(frame, area, pending);
        }

        // Destructive-statement confirmation modal.
        if let Some(dangerous) = &self.pending_dangerous {
            render_dangerous_modal(frame, area, dangerous);
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

        // Transaction indicator: only an open session transaction is flagged
        // (amber). Autocommit is the implicit default and shows nothing, which
        // keeps the status line within its width budget.
        let txn_active = self
            .connection_name
            .as_deref()
            .and_then(|name| self.executors.get(name))
            .is_some_and(|exec| exec.in_transaction());
        let txn_span = if txn_active {
            Span::styled("txn: ACTIVE ", Style::default().fg(Color::Rgb(255, 176, 0)))
        } else {
            Span::raw("")
        };

        let error_span = if let Some(ref err) = self.last_error {
            Span::styled(format!(" ERR: {} │ ", err), Style::default().fg(Color::Red))
        } else if let Some(ref notice) = self.last_notice {
            Span::styled(format!(" {} │ ", notice), Style::default().fg(Color::Green))
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
            " <Space>e editor │ <Space>h history │ <Space>r recent │ <Space>x export │ <Space>i import │ <C-q> quit "
        };
        let hint_span = Span::styled(hint, Style::default().fg(Color::DarkGray));

        let status = Line::from(vec![
            mode_span, conn_span, txn_span, error_span, stats_span, edit_span, hint_span,
        ]);
        let status_bar = Paragraph::new(status).style(Style::default().bg(Color::Black));
        frame.render_widget(status_bar, outer[1]);
    }

    fn handle_key_event(&mut self, key: KeyEvent, tx: &UnboundedSender<AppMsg>) {
        if key.kind != KeyEventKind::Press {
            return;
        }

        tracing::debug!("key: {:?}, modifiers: {:?}", key.code, key.modifiers);

        // Save-as filename prompt swallows keys until confirmed/cancelled.
        if self.save_prompt.is_some() {
            match key.code {
                KeyCode::Enter => self.confirm_save_prompt(),
                KeyCode::Esc => self.save_prompt = None,
                KeyCode::Backspace => {
                    if let Some(name) = self.save_prompt.as_mut() {
                        name.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(name) = self.save_prompt.as_mut() {
                        name.push(c);
                    }
                }
                _ => {}
            }
            return;
        }

        // Quit-with-unsaved-buffers prompt.
        if self.quit_prompt {
            match key.code {
                KeyCode::Char('d') => self.should_quit = true,
                KeyCode::Char('c') | KeyCode::Esc => self.quit_prompt = false,
                KeyCode::Char('s') => {
                    self.quit_prompt = false;
                    self.save_editor();
                    if self.save_prompt.is_none() && !self.editor.any_dirty() {
                        self.should_quit = true;
                    }
                }
                _ => {}
            }
            return;
        }

        // Import file-path prompt swallows keys until confirmed/cancelled.
        if self.import_prompt.is_some() {
            match key.code {
                KeyCode::Enter => self.confirm_import_prompt(),
                KeyCode::Esc => self.import_prompt = None,
                KeyCode::Backspace => {
                    if let Some(p) = self.import_prompt.as_mut() {
                        p.path.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(p) = self.import_prompt.as_mut() {
                        p.path.push(c);
                    }
                }
                _ => {}
            }
            return;
        }

        // Import confirmation modal swallows keys until dismissed.
        if self.pending_import.is_some() {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') => self.confirm_import(tx),
                KeyCode::Esc | KeyCode::Char('n') => self.pending_import = None,
                _ => {}
            }
            return;
        }

        // Modal list picker (history / recent files) swallows keys until closed.
        if self.picker.is_some() {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => self.picker_move(1),
                KeyCode::Char('k') | KeyCode::Up => self.picker_move(-1),
                KeyCode::Enter => self.picker_select(tx),
                KeyCode::Esc | KeyCode::Char('q') => self.picker = None,
                _ => {}
            }
            return;
        }

        // Destructive-statement confirmation swallows keys until dismissed.
        if self.pending_dangerous.is_some() {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') => self.confirm_dangerous(tx),
                KeyCode::Esc | KeyCode::Char('n') => self.pending_dangerous = None,
                _ => {}
            }
            return;
        }

        if self.editor_open {
            let (new_mode, action) = self.editor.handle_key(key, self.mode);
            tracing::debug!("editor action: {:?}, new_mode: {:?}", action, new_mode);
            self.mode = new_mode;
            match action {
                EditorAction::Execute => self.run_editor_sql(tx),
                EditorAction::Save => self.save_editor(),
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
                if self.editor.any_dirty() {
                    self.quit_prompt = true;
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Tree => Focus::Grid,
                    Focus::Grid => Focus::Tree,
                };
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('h') if self.pending_leader => {
                self.open_history(tx);
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('r') if self.pending_leader => {
                self.open_recent_files(tx);
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('x') if self.pending_leader => {
                self.open_export_menu();
                self.pending_leader = false;
                self.pending_g = false;
            }
            KeyCode::Char('i') if self.pending_leader => {
                self.start_import();
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
                self.last_notice = None;
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
                    self.run_sql(&name, sql, tx, false);
                }
            }
            AppMsg::CommitResult(Err(error)) => {
                tracing::warn!("commit error: {}", error);
                // Keep pending edits so the user can fix and retry.
                self.last_error = Some(error);
            }
            AppMsg::HistoryLoaded(entries) => {
                let items = entries
                    .into_iter()
                    .map(|h| {
                        let first = h.sql.lines().next().unwrap_or("").trim();
                        let dur = h
                            .duration_ms
                            .map(|d| format!("{d}ms"))
                            .unwrap_or_else(|| "-".into());
                        let mark = if h.error.is_some() { "✗" } else { " " };
                        let label =
                            format!("{mark} [{}] {} ({dur})", h.connection, truncate(first, 60));
                        PickerItem {
                            label,
                            action: PickerAction::LoadSql(h.sql),
                        }
                    })
                    .collect();
                self.picker = Some(Picker {
                    title: "Query history".into(),
                    items,
                    selected: 0,
                });
            }
            AppMsg::RecentFilesLoaded(entries) => {
                let items = entries
                    .into_iter()
                    .map(|f| PickerItem {
                        label: format!("{}  ({})", f.path, f.last_opened),
                        action: PickerAction::OpenFile(std::path::PathBuf::from(f.path)),
                    })
                    .collect();
                self.picker = Some(Picker {
                    title: "Recent files".into(),
                    items,
                    selected: 0,
                });
            }
            AppMsg::ExportFinished(Ok(path)) => {
                tracing::info!("exported to {}", path.display());
                self.last_error = None;
                self.last_notice = Some(format!("exported → {}", path.display()));
            }
            AppMsg::ExportFinished(Err(error)) => {
                tracing::warn!("export error: {}", error);
                self.last_error = Some(format!("export failed: {error}"));
            }
            AppMsg::ImportFinished(Ok(affected)) => {
                tracing::info!("import affected {affected} rows");
                self.last_error = None;
                self.last_notice = Some(format!("imported {affected} rows"));
                // Refresh the current browse so imported rows appear.
                if let (Some(name), Some(sql)) =
                    (self.connection_name.clone(), self.last_browse_sql.clone())
                {
                    self.run_sql(&name, sql, tx, false);
                }
            }
            AppMsg::ImportFinished(Err(error)) => {
                tracing::warn!("import error: {}", error);
                self.last_error = Some(format!("import failed: {error}"));
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
        // Snapshot the active buffer so reopening the editor for this
        // connection restores its text (volatile, in-memory convenience).
        if let Some(name) = self.connection_name.clone() {
            self.saved_buffers.insert(name, self.editor.content());
        }
        self.editor_open = false;
        self.mode = Mode::Normal;
    }

    /// Save the active buffer to its `.sql` file, prompting for a name on the
    /// first save.
    fn save_editor(&mut self) {
        match self.editor.active_path() {
            Some(path) => self.write_active_buffer(&path),
            None => self.save_prompt = Some(String::new()),
        }
    }

    /// Resolve the typed filename, write the active buffer, and bind the path.
    fn confirm_save_prompt(&mut self) {
        let Some(name) = self.save_prompt.take() else {
            return;
        };
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let path = sextant_config::query_path(name);
        self.write_active_buffer(&path);
    }

    fn write_active_buffer(&mut self, path: &std::path::Path) {
        match sextant_config::write_query(path, &self.editor.content()) {
            Ok(()) => {
                self.editor.set_active_path(path.to_path_buf());
                self.editor.mark_saved();
                self.last_error = None;
                self.record_recent_file(path);
            }
            Err(e) => {
                self.last_error = Some(format!("save failed: {e}"));
            }
        }
    }

    fn run_editor_sql(&mut self, tx: &UnboundedSender<AppMsg>) {
        let sql = self.editor.content();
        let Some(name) = self.connection_name.clone() else {
            tracing::warn!("run_editor_sql: no connection_name");
            return;
        };
        // Destructive statements (DELETE/UPDATE without WHERE, DDL) require a
        // confirmation step before they run.
        if let Some(reason) = sextant_db::dangerous_reason(&sql) {
            self.pending_dangerous = Some(DangerousStmt {
                conn: name,
                sql,
                reason,
            });
            return;
        }
        self.execute_editor_sql(name, sql, tx);
    }

    /// Run an editor statement: clear the (read-only) edit context, drop the
    /// browse-replay SQL, and dispatch it to the executor (recorded in history).
    fn execute_editor_sql(&mut self, name: String, sql: String, tx: &UnboundedSender<AppMsg>) {
        // Ad-hoc editor results are not tied to a browseable table → read-only.
        self.result_grid.set_edit_context(None);
        self.last_browse_sql = None;
        self.run_sql(&name, sql, tx, true);
    }

    /// Run the confirmed destructive statement.
    fn confirm_dangerous(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(d) = self.pending_dangerous.take() else {
            return;
        };
        self.execute_editor_sql(d.conn, d.sql, tx);
    }

    /// Spawn a query against the named connection's executor; the result is
    /// delivered back through the channel as `QueryResult`/`QueryError`.
    ///
    /// When `record` is set, the statement (with its duration and any error) is
    /// appended to the query history. Auto-generated queries (table browse,
    /// post-commit refresh) pass `record = false` to keep the history clean.
    fn run_sql(
        &mut self,
        conn_name: &str,
        sql: String,
        tx: &UnboundedSender<AppMsg>,
        record: bool,
    ) {
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
        let store = if record {
            self.state_store.clone()
        } else {
            None
        };
        let conn = conn_name.to_string();
        tokio::spawn(async move {
            let started = Instant::now();
            let outcome = executor.execute(&sql).await;

            if let Some(store) = store {
                let duration_ms = Some(started.elapsed().as_millis() as i64);
                let error = outcome.as_ref().err().map(|e| e.to_string());
                let _ = store
                    .record_query(&conn, &sql, duration_ms, error.as_deref())
                    .await;
            }

            match outcome {
                Ok(result) => {
                    let _ = tx.send(AppMsg::QueryResult(result));
                }
                Err(e) => {
                    let _ = tx.send(AppMsg::QueryError(format!("{e}")));
                }
            }
        });
    }

    /// Record a saved file in the recent-files ring (fire-and-forget).
    fn record_recent_file(&self, path: &std::path::Path) {
        if let (Some(store), Some(conn)) = (self.state_store.clone(), self.connection_name.clone())
        {
            let path = path.to_string_lossy().into_owned();
            tokio::spawn(async move {
                let _ = store.record_file(&conn, &path).await;
            });
        }
    }

    /// Load the query history into a modal picker (async fetch → `HistoryLoaded`).
    fn open_history(&self, tx: &UnboundedSender<AppMsg>) {
        let Some(store) = self.state_store.clone() else {
            return;
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Ok(items) = store.recent_queries(50).await {
                let _ = tx.send(AppMsg::HistoryLoaded(items));
            }
        });
    }

    /// Load the active connection's recent files into a modal picker.
    fn open_recent_files(&self, tx: &UnboundedSender<AppMsg>) {
        let (Some(store), Some(conn)) = (self.state_store.clone(), self.connection_name.clone())
        else {
            return;
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Ok(items) = store.recent_files(&conn).await {
                let _ = tx.send(AppMsg::RecentFilesLoaded(items));
            }
        });
    }

    /// Open the export-format picker for the current result set.
    fn open_export_menu(&mut self) {
        if self.last_result.is_none() {
            self.last_error = Some("nothing to export".into());
            return;
        }
        let items = [
            sextant_db::ExportFormat::Csv,
            sextant_db::ExportFormat::Json,
            sextant_db::ExportFormat::Sql,
        ]
        .into_iter()
        .map(|f| PickerItem {
            label: f.label().to_string(),
            action: PickerAction::Export(f),
        })
        .collect();
        self.picker = Some(Picker {
            title: "Export as".into(),
            items,
            selected: 0,
        });
    }

    /// Serialize the current result set in `format` and write it to a
    /// timestamped file under the exports directory (async; reports back via
    /// [`AppMsg::ExportFinished`]).
    fn run_export(&mut self, format: sextant_db::ExportFormat, tx: &UnboundedSender<AppMsg>) {
        let Some(result) = self.last_result.clone() else {
            return;
        };
        // SQL export needs a target table name and dialect; fall back to a
        // generic name for ad-hoc query results that aren't a single table.
        let driver = self
            .connection_name
            .as_deref()
            .and_then(|c| self.driver_for(c))
            .unwrap_or(sextant_core::Driver::Postgres);
        let table = self
            .result_grid
            .edit_context()
            .map(|c| c.table.clone())
            .unwrap_or_else(|| "exported_data".to_string());

        let stem = self
            .result_grid
            .edit_context()
            .map(|c| c.table.clone())
            .unwrap_or_else(|| "export".to_string());
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let path = sextant_config::exports_dir()
            .join(format!("{stem}-{millis}.{ext}", ext = format.extension()));

        let tx = tx.clone();
        tokio::spawn(async move {
            let content = sextant_db::export::export(&result, format, driver, &table);
            let msg = match sextant_config::write_export(&path, &content) {
                Ok(()) => AppMsg::ExportFinished(Ok(path)),
                Err(e) => AppMsg::ExportFinished(Err(e.to_string())),
            };
            let _ = tx.send(msg);
        });
    }

    /// Begin importing into the table selected in the tree: open a file-path
    /// prompt bound to that target.
    fn start_import(&mut self) {
        let Some(tree_pane::LineKind::Table {
            conn,
            schema,
            table,
        }) = self.tree.selected_kind()
        else {
            self.last_error = Some("select a table in the tree to import into".into());
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
        self.import_prompt = Some(ImportPrompt {
            conn: conn_name,
            schema: schema_name,
            table: table_name,
            driver,
            path: String::new(),
        });
    }

    /// Read the prompted file, parse it by extension, and stage the resulting
    /// statements for confirmation. Relative paths resolve under the exports
    /// directory (symmetric with where exports are written).
    fn confirm_import_prompt(&mut self) {
        let Some(prompt) = self.import_prompt.take() else {
            return;
        };
        let raw = std::path::PathBuf::from(prompt.path.trim());
        let path = if raw.is_absolute() {
            raw
        } else {
            sextant_config::exports_dir().join(raw)
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                self.last_error = Some(format!("import: cannot read {}: {e}", path.display()));
                return;
            }
        };

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let (statements, summary) = match ext.as_str() {
            "csv" | "json" => {
                let parsed = if ext == "csv" {
                    sextant_db::import::parse_csv(&content)
                } else {
                    sextant_db::import::parse_json(&content)
                };
                let data = match parsed {
                    Ok(d) => d,
                    Err(e) => {
                        self.last_error = Some(format!("import: {e}"));
                        return;
                    }
                };
                let Some(meta) = self
                    .table_meta
                    .get(&prompt.conn)
                    .and_then(|m| m.get(&(prompt.schema.clone(), prompt.table.clone())))
                else {
                    self.last_error =
                        Some("import: no column metadata for the target table".into());
                    return;
                };
                let preview = sextant_db::import::preview(&data, meta);
                let statements = sextant_db::import::build_inserts(
                    prompt.driver,
                    &prompt.schema,
                    &prompt.table,
                    &data,
                    &preview.mapping,
                    meta,
                );
                if statements.is_empty() {
                    self.last_error =
                        Some("import: no source columns match the target table".into());
                    return;
                }
                (statements, import_summary(&prompt.table, &preview))
            }
            "sql" => {
                let statements = sextant_db::import::split_sql_statements(&content);
                if statements.is_empty() {
                    self.last_error = Some("import: the SQL file has no statements".into());
                    return;
                }
                let summary = vec![format!(
                    "Run {} SQL statement(s) as a script.",
                    statements.len()
                )];
                (statements, summary)
            }
            _ => {
                self.last_error =
                    Some("import: unsupported file type (use .csv/.json/.sql)".into());
                return;
            }
        };

        self.pending_import = Some(PendingImport {
            conn: prompt.conn,
            summary,
            statements,
        });
    }

    /// Run the staged import statements in a transaction (async; reports back
    /// via [`AppMsg::ImportFinished`]).
    fn confirm_import(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(pending) = self.pending_import.take() else {
            return;
        };
        let Some(executor) = self.executors.get(&pending.conn).cloned() else {
            self.last_error = Some("import: not connected".into());
            return;
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            let msg = match executor.execute_transaction(&pending.statements).await {
                Ok(affected) => AppMsg::ImportFinished(Ok(affected)),
                Err(e) => AppMsg::ImportFinished(Err(e.to_string())),
            };
            let _ = tx.send(msg);
        });
    }

    /// Move the picker selection by `delta`, wrapping around.
    fn picker_move(&mut self, delta: isize) {
        if let Some(p) = self.picker.as_mut() {
            if p.items.is_empty() {
                return;
            }
            let len = p.items.len() as isize;
            p.selected = (p.selected as isize + delta).rem_euclid(len) as usize;
        }
    }

    /// Act on the highlighted picker entry, then dismiss the picker.
    fn picker_select(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(picker) = self.picker.take() else {
            return;
        };
        let Picker {
            items, selected, ..
        } = picker;
        let Some(item) = items.into_iter().nth(selected) else {
            return;
        };
        match item.action {
            PickerAction::LoadSql(sql) => self.load_into_editor(&sql, None),
            PickerAction::OpenFile(path) => match std::fs::read_to_string(&path) {
                Ok(content) => self.load_into_editor(&content, Some(path)),
                Err(e) => self.last_error = Some(format!("open failed: {e}")),
            },
            PickerAction::Export(format) => self.run_export(format, tx),
        }
    }

    /// Open the editor with the given content; when `path` is set, bind it as
    /// the buffer's file (a freshly-loaded file is not dirty).
    fn load_into_editor(&mut self, content: &str, path: Option<std::path::PathBuf>) {
        self.open_editor();
        self.editor.set_content(content);
        if let Some(path) = path {
            self.editor.set_active_path(path);
            self.editor.mark_saved();
        }
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
        self.run_sql(&conn_name, sql, tx, false);
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

    // Open the local state store; the app degrades gracefully if it fails.
    match sextant_state::StateStore::open(&sextant_config::state_db_path()).await {
        Ok(store) => app.state_store = Some(store),
        Err(e) => tracing::warn!("state store unavailable: {e}"),
    }

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

/// Render a small centered modal with the given title and body lines.
fn render_centered_modal(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line>) {
    let width = (area.width as f32 * 0.6).max(20.0) as u16;
    let height = (lines.len() as u16 + 2).min(area.height.max(3));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(format!(" {title} "))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .style(Style::default().bg(Color::Black)),
        ),
        rect,
    );
}

/// Build the read-only summary lines shown before a CSV/JSON import runs.
fn import_summary(table: &str, preview: &sextant_db::ImportPreview) -> Vec<String> {
    let mut lines = vec![format!(
        "Insert {} row(s) into {}.",
        preview.row_count, table
    )];
    let mapped: Vec<&str> = preview
        .mapping
        .pairs
        .iter()
        .map(|(_, t)| t.as_str())
        .collect();
    lines.push(format!("Columns: {}", mapped.join(", ")));
    if !preview.mapping.unmatched_source.is_empty() {
        lines.push(format!(
            "Ignored (no match): {}",
            preview.mapping.unmatched_source.join(", ")
        ));
    }
    if preview.type_issues > 0 {
        lines.push(format!(
            "⚠ {} value(s) may not match the column type",
            preview.type_issues
        ));
    }
    lines
}

/// Render the import file-path prompt.
fn render_import_prompt(frame: &mut Frame, area: Rect, prompt: &ImportPrompt) {
    render_centered_modal(
        frame,
        area,
        &format!("Import into {}", prompt.table),
        vec![
            Line::from(format!("{}_", prompt.path)),
            Line::from(Span::styled(
                "path to .csv/.json/.sql (abs, or under exports dir) │ <Enter> load │ <Esc> cancel",
                Style::default().fg(Color::DarkGray),
            )),
        ],
    );
}

/// Render the import-confirmation modal listing the summary of what will run.
fn render_import_modal(frame: &mut Frame, area: Rect, pending: &PendingImport) {
    let mut lines: Vec<Line> = pending
        .summary
        .iter()
        .map(|s| Line::from(Span::raw(s.clone())))
        .collect();
    lines.push(Line::from(Span::styled(
        "<Enter>/y import   <Esc>/n cancel",
        Style::default().fg(Color::DarkGray),
    )));
    render_centered_modal(frame, area, "Confirm import", lines);
}

/// Render the destructive-statement confirmation modal.
fn render_dangerous_modal(frame: &mut Frame, area: Rect, dangerous: &DangerousStmt) {
    let first = dangerous.sql.lines().next().unwrap_or("").trim();
    render_centered_modal(
        frame,
        area,
        "Confirm destructive statement",
        vec![
            Line::from(Span::styled(
                format!("⚠ {}", dangerous.reason),
                Style::default().fg(Color::Red),
            )),
            Line::from(Span::raw(format!("  {}", truncate(first, 70)))),
            Line::from(Span::styled(
                "<Enter>/y run   <Esc>/n cancel",
                Style::default().fg(Color::DarkGray),
            )),
        ],
    );
}

/// Render the Save-as filename prompt.
fn render_save_prompt(frame: &mut Frame, area: Rect, name: &str) {
    render_centered_modal(
        frame,
        area,
        "Save as",
        vec![
            Line::from(format!("{name}_")),
            Line::from(Span::styled(
                "type a name (.sql) │ <Enter> save │ <Esc> cancel",
                Style::default().fg(Color::DarkGray),
            )),
        ],
    );
}

/// Truncate `s` to at most `max` chars, appending `…` when shortened.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Render a modal list picker (query history / recent files).
fn render_picker(frame: &mut Frame, area: Rect, picker: &Picker) {
    let width = (area.width as f32 * 0.7).max(20.0) as u16;
    let height = ((picker.items.len() as u16).max(1) + 3).min(area.height.max(6));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    // Rows that fit inside the borders, minus the footer hint line.
    let max_rows = height.saturating_sub(3) as usize;
    let offset = picker.selected.saturating_sub(max_rows.saturating_sub(1));

    let mut lines: Vec<Line> = if picker.items.is_empty() {
        vec![Line::from(Span::styled(
            "(empty)",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        picker
            .items
            .iter()
            .enumerate()
            .skip(offset)
            .take(max_rows)
            .map(|(i, item)| {
                if i == picker.selected {
                    Line::from(Span::styled(
                        format!("▶ {}", item.label),
                        Style::default().fg(Color::Black).bg(Color::Cyan),
                    ))
                } else {
                    Line::from(Span::raw(format!("  {}", item.label)))
                }
            })
            .collect()
    };
    lines.push(Line::from(Span::styled(
        "<j/k> move │ <Enter> open │ <Esc> close",
        Style::default().fg(Color::DarkGray),
    )));

    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(format!(" {} ", picker.title))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .style(Style::default().bg(Color::Black)),
        ),
        rect,
    );
}

/// Render the quit-with-unsaved-buffers prompt.
fn render_quit_prompt(frame: &mut Frame, area: Rect) {
    render_centered_modal(
        frame,
        area,
        "Unsaved buffers",
        vec![
            Line::from("There are unsaved buffers."),
            Line::from(Span::styled(
                "s save │ d discard & quit │ c/<Esc> cancel",
                Style::default().fg(Color::DarkGray),
            )),
        ],
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
    fn dangerous_editor_sql_requires_confirmation() {
        let mut app = test_app();
        app.connection_name = Some("c".into());
        app.editor.set_content("DROP TABLE users");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.run_editor_sql(&tx);
        assert!(
            app.pending_dangerous.is_some(),
            "DDL should stage a confirmation modal"
        );
    }

    #[test]
    fn safe_editor_sql_runs_without_prompt() {
        let mut app = test_app();
        app.connection_name = Some("c".into());
        app.editor.set_content("SELECT 1");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.run_editor_sql(&tx);
        assert!(
            app.pending_dangerous.is_none(),
            "a plain SELECT should not prompt"
        );
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

        // Return to normal mode, then close the modal (which snapshots the
        // buffer for this connection).
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
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

    #[test]
    fn ctrl_s_opens_save_prompt_then_esc_cancels() {
        let mut app = test_app();
        app.open_editor();
        // Insert mode, type something so the buffer is dirty.
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('i'))));
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('x'))));
        // Ctrl+S on an unsaved buffer opens the Save-as prompt.
        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Char('s'),
            KeyModifiers::CONTROL,
        )));
        assert!(app.save_prompt.is_some());

        // Typing feeds the filename buffer.
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('q'))));
        assert_eq!(app.save_prompt.as_deref(), Some("q"));

        // Esc cancels without writing, editor stays open.
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Esc)));
        assert!(app.save_prompt.is_none());
        assert!(app.editor_open);
    }

    #[test]
    fn ctrl_q_with_dirty_buffer_prompts() {
        let mut app = test_app();
        app.open_editor();
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('i'))));
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('x'))));
        // Esc to Normal, Esc to close the modal (buffer remains dirty/unsaved).
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Esc)));
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Esc)));
        assert!(!app.editor_open);

        // Ctrl+Q prompts instead of quitting because a buffer is dirty.
        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
        )));
        assert!(app.quit_prompt);
        assert!(!app.should_quit);

        // `c` cancels the prompt.
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('c'))));
        assert!(!app.quit_prompt);
        assert!(!app.should_quit);
    }

    fn picker_with(actions: Vec<PickerAction>) -> Picker {
        Picker {
            title: "test".into(),
            items: actions
                .into_iter()
                .enumerate()
                .map(|(i, action)| PickerItem {
                    label: format!("item {i}"),
                    action,
                })
                .collect(),
            selected: 0,
        }
    }

    #[test]
    fn picker_navigation_wraps() {
        let mut app = test_app();
        app.picker = Some(picker_with(vec![
            PickerAction::LoadSql("a".into()),
            PickerAction::LoadSql("b".into()),
        ]));

        // k from the first item wraps to the last.
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('k'))));
        assert_eq!(app.picker.as_ref().unwrap().selected, 1);
        // j from the last item wraps back to the first.
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('j'))));
        assert_eq!(app.picker.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn picker_select_loads_sql_into_editor() {
        let mut app = test_app();
        app.picker = Some(picker_with(vec![
            PickerAction::LoadSql("SELECT 1".into()),
            PickerAction::LoadSql("SELECT 2".into()),
        ]));

        // Move to the second entry and select it.
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('j'))));
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Enter)));

        assert!(app.picker.is_none(), "picker should close after selection");
        assert!(app.editor_open, "selecting loads the query into the editor");
        assert_eq!(app.editor.content(), "SELECT 2");
    }

    #[test]
    fn picker_esc_dismisses_without_action() {
        let mut app = test_app();
        app.picker = Some(picker_with(vec![PickerAction::LoadSql("SELECT 1".into())]));
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Esc)));
        assert!(app.picker.is_none());
        assert!(!app.editor_open);
    }

    #[test]
    fn truncate_shortens_long_strings() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
    }
}
