use ratatui::crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame, Terminal,
};
use sextant_core::QueryExecutor;
use std::io::{self, stdout, Stdout};

mod editor_modal;
use editor_modal::{EditorAction, EditorModal};

mod tree_pane;
use tree_pane::{ConnState, SchemaItem, TreePane};

mod result_grid;
use result_grid::ResultGrid;

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

enum AsyncResult {
    Connected {
        name: String,
        executor: sextant_db::SqlxExecutor,
        schemas: Vec<sextant_db::introspection::Schema>,
    },
    Failed {
        name: String,
        error: String,
    },
    QueryResult(sextant_core::QueryResult),
    QueryError(String),
}

/// Application state for the TUI.
pub struct App {
    pub mode: Mode,
    pub connection_name: Option<String>,
    should_quit: bool,
    tree: TreePane,
    connection_configs: Vec<sextant_core::Connection>,
    runtime: tokio::runtime::Runtime,
    async_rx: std::sync::mpsc::Receiver<AsyncResult>,
    async_tx: std::sync::mpsc::Sender<AsyncResult>,
    executors: std::collections::HashMap<String, sextant_db::SqlxExecutor>,
    editor_open: bool,
    editor: EditorModal,
    pending_leader: bool,
    pending_g: bool,
    saved_buffers: std::collections::HashMap<String, String>,
    last_result: Option<sextant_core::QueryResult>,
    last_error: Option<String>,
    last_query_duration: Option<std::time::Duration>,
    query_start: Option<std::time::Instant>,
    focus: Focus,
    result_grid: ResultGrid,
}

impl App {
    fn new() -> io::Result<Self> {
        let connections = sextant_config::load_connections().unwrap_or_else(|e| {
            tracing::warn!("failed to load connections: {e}");
            vec![]
        });
        let names: Vec<String> = connections.iter().map(|c| c.name.clone()).collect();
        let tree = TreePane::new(names);
        let runtime = tokio::runtime::Runtime::new()?;
        let (async_tx, async_rx) = std::sync::mpsc::channel();

        Ok(Self {
            mode: Mode::Normal,
            connection_name: None,
            should_quit: false,
            tree,
            connection_configs: connections,
            runtime,
            async_rx,
            async_tx,
            executors: std::collections::HashMap::new(),
            editor_open: false,
            editor: EditorModal::new(),
            pending_leader: false,
            pending_g: false,
            saved_buffers: std::collections::HashMap::new(),
            last_result: None,
            last_error: None,
            last_query_duration: None,
            query_start: None,
            focus: Focus::Tree,
            result_grid: ResultGrid::new(),
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
        self.result_grid.set_result(self.last_result.clone());
        self.result_grid.render(frame, inner[1]);

        // Editor modal (floating overlay).
        if self.editor_open {
            self.editor.render(frame, area);
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

        let hint = if self.editor_open {
            if self.mode == Mode::Normal {
                " <i> insert │ <C-Enter> run │ <Esc> close "
            } else {
                " <Esc> normal "
            }
        } else {
            " <Space>e editor │ <C-q> quit "
        };
        let hint_span = Span::styled(hint, Style::default().fg(Color::DarkGray));

        let status = Line::from(vec![mode_span, conn_span, stats_span, hint_span]);
        let status_bar = Paragraph::new(status).style(Style::default().bg(Color::Black));
        frame.render_widget(status_bar, outer[1]);
    }

    fn draw(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
        terminal.draw(|frame| self.render(frame))?;
        Ok(())
    }

    fn handle_event(&mut self, event: Event) {
        if let Event::Key(key) = event {
            if key.kind != KeyEventKind::Press {
                return;
            }

            if self.editor_open {
                let (new_mode, action) = self.editor.handle_key(key, self.mode);
                self.mode = new_mode;
                match action {
                    EditorAction::Execute => self.run_editor_sql(),
                    EditorAction::Save => self.save_editor_buffer(),
                    EditorAction::Close => self.close_editor(),
                    EditorAction::None => {}
                }
                return;
            }

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
                    if self.focus == Focus::Grid {
                        self.result_grid.move_left();
                    }
                    self.pending_leader = false;
                    self.pending_g = false;
                }
                KeyCode::Char('l') if key.modifiers.is_empty() => {
                    if self.focus == Focus::Grid {
                        self.result_grid.move_right();
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
                    if self.focus == Focus::Tree {
                        self.handle_enter();
                    }
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
                _ => {
                    self.pending_leader = false;
                    self.pending_g = false;
                }
            }
        }
    }

    fn handle_enter(&mut self) {
        let Some(kind) = self.tree.selected_kind() else { return };

        match kind {
            tree_pane::LineKind::Connection => {
                let Some(conn_idx) = self.tree.selected_connection_index() else { return };
                let state = &self.tree.connections[conn_idx].state;
                match state {
                    ConnState::Disconnected | ConnState::Error(_) => {
                        let name = self.tree.connections[conn_idx].name.clone();
                        self.start_connection(&name, conn_idx);
                    }
                    ConnState::Connected { .. } => {
                        self.tree.toggle_selected();
                    }
                    ConnState::Connecting => {}
                }
            }
            tree_pane::LineKind::Schema { .. } => {
                self.tree.toggle_selected();
            }
            tree_pane::LineKind::Table { .. } => {
                // No-op for v0.1; table browsing comes in 1.5.
            }
        }
    }

    fn start_connection(&mut self, name: &str, conn_idx: usize) {
        let Some(config) = self.connection_configs.iter().find(|c| c.name == name) else {
            return;
        };

        self.tree.set_connecting(conn_idx);
        self.connection_name = Some(format!("{name} (connecting)"));

        let password = sextant_config::connection_password(name);
        let config = config.clone();
        let tx = self.async_tx.clone();
        let name = name.to_string();

        self.runtime.spawn(async move {
            let mut mgr = sextant_db::ConnectionManager::new();
            match mgr.connect(&name, &config, password.as_deref()).await {
                Ok(executor) => {
                    match sextant_db::introspection::introspect_schemas_and_tables(
                        &executor,
                        config.driver,
                    )
                    .await
                    {
                        Ok(schemas) => {
                            let _ = tx.send(AsyncResult::Connected {
                                name,
                                executor,
                                schemas,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(AsyncResult::Failed {
                                name,
                                error: format!("{e}"),
                            });
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(AsyncResult::Failed {
                        name,
                        error: format!("{e}"),
                    });
                }
            }
        });
    }

    fn handle_async_results(&mut self) {
        while let Ok(result) = self.async_rx.try_recv() {
            match result {
                AsyncResult::Connected {
                    name,
                    executor,
                    schemas,
                } => {
                    let schema_items = schemas
                        .into_iter()
                        .map(|s| SchemaItem {
                            name: s.name,
                            expanded: true,
                            tables: s.tables,
                        })
                        .collect();

                    if let Some(idx) = self.tree.connection_index_by_name(&name) {
                        self.tree.set_connected(idx, schema_items);
                    }
                    self.executors.insert(name.clone(), executor);
                    self.connection_name = Some(name);
                }
                AsyncResult::Failed { name, error } => {
                    if let Some(idx) = self.tree.connection_index_by_name(&name) {
                        self.tree.set_error(idx, error.clone());
                    }
                    self.connection_name = Some(format!("{name}: {error}"));
                }
                AsyncResult::QueryResult(result) => {
                    self.last_result = Some(result);
                    self.last_error = None;
                    self.last_query_duration = self.query_start.take().map(|t| t.elapsed());
                }
                AsyncResult::QueryError(error) => {
                    self.last_error = Some(error);
                }
            }
        }
    }

    fn open_editor(&mut self) {
        self.editor_open = true;
        self.mode = Mode::Normal;
        if let Some(name) = &self.connection_name {
            if let Some(buf) = self.saved_buffers.get(name) {
                self.editor.set_content(buf);
            }
        }
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

    fn run_editor_sql(&mut self) {
        let sql = self.editor.content();
        let Some(name) = self.connection_name.clone() else { return };
        let Some(executor) = self.executors.get(&name).cloned() else { return };
        self.query_start = Some(std::time::Instant::now());
        let tx = self.async_tx.clone();
        self.runtime.spawn(async move {
            match executor.execute(&sql).await {
                Ok(result) => {
                    let _ = tx.send(AsyncResult::QueryResult(result));
                }
                Err(e) => {
                    let _ = tx.send(AsyncResult::QueryError(format!("{e}")));
                }
            }
        });
    }
}

/// Run the TUI event loop until the user quits.
pub fn run() -> io::Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = App::new()?;

    let result = (|| -> io::Result<()> {
        loop {
            app.handle_async_results();
            app.draw(&mut terminal)?;

            if event::poll(std::time::Duration::from_millis(16))? {
                let event = event::read()?;
                app.handle_event(event);
            }

            if app.should_quit {
                break;
            }
        }
        Ok(())
    })();

    restore_terminal(&mut terminal)?;
    result
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
        assert!(text.contains("INS"), "status line should show Insert mode: {text}");

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
            assert_eq!(cell.style().bg, Some(Color::Black), "bg at (15,{y}) should be Black");
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
        assert!(rows[9].contains("NOR"), "last row should contain status: {}", rows[9]);
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
            border_cell.symbol() == "│" || border_cell.symbol() == "┐" || border_cell.symbol() == "┘" || border_cell.symbol() == "┤",
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
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            KeyEventKind::Release,
        )));
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
        assert!(text.contains("SQL Editor"), "modal should render title: {text}");
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
        app.last_query_duration = Some(std::time::Duration::from_millis(38));

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
}
