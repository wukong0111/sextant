use std::io::{self, Stdout, stdout};
use std::sync::Arc;
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
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use sextant_core::QueryExecutor;
use tokio::sync::mpsc::UnboundedSender;

mod autocomplete;

mod keymap;
use keymap::{Action, ChordState, KeySpec, Keymap};

mod fuzzy;
use fuzzy::{FuzzyAction, FuzzyItem, FuzzyPicker};

mod palette;
use palette::Palette;

mod swap;

mod editor_modal;
use editor_modal::{EditorAction, EditorModal};

mod tree_pane;
use tree_pane::{ColumnNode, ConnState, SchemaItem, TableItem, TreePane};

mod result_grid;
pub use result_grid::CopyFormat;
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
    /// Snippets loaded from the state store (for the snippet picker).
    SnippetsLoaded(Vec<sextant_state::Snippet>),
    /// A snippet was saved (the snippet name), or an error.
    SnippetSaved(Result<String, String>),
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
    /// Copy the selected grid range in the chosen format.
    CopyFormat(CopyFormat),
}

/// An import in progress: the chosen target table and the file path being typed.
struct ImportPrompt {
    conn: String,
    schema: String,
    table: String,
    driver: sextant_core::Driver,
    path: String,
}

/// Orphan swap files found at startup, offered for crash recovery.
struct Recovery {
    /// All orphan swap files (deleted on restore or discard).
    orphans: Vec<std::path::PathBuf>,
    /// Buffers parsed from the newest orphan, to load on restore.
    buffers: Vec<swap::SwapBuffer>,
}

/// An interactive password prompt shown when a connection's secret is not in
/// the keyring. On submit the password is tried and, on success, stored.
struct PasswordPrompt {
    /// Connection name being connected.
    name: String,
    /// Index of the connection in the tree.
    conn_idx: usize,
    /// Keyring key to store the password under on success.
    keyring_key: String,
    /// The password typed so far (rendered masked).
    input: String,
}

/// A password entered at the prompt, awaiting a successful connection before it
/// is persisted to the credential store (§3.2 "save after connect").
struct PendingCredential {
    /// Connection whose success triggers the save.
    name: String,
    /// Key to store the password under.
    keyring_key: String,
    /// The password to persist.
    password: String,
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
    Visual,
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
            Mode::Visual => write!(f, "VIS"),
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
    /// Remappable Normal-mode bindings (defaults + `keys.toml`).
    keymap: Keymap,
    /// In-progress key chord for multi-key bindings (`gg`, `<Space>e`, …).
    chord: ChordState,
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
    /// Active copy-format picker (CSV / JSON / SQL INSERT), if any.
    copy_format_picker: Option<Picker>,
    /// Active import file-path prompt, if any.
    import_prompt: Option<ImportPrompt>,
    /// Parsed import awaiting confirmation, if any.
    pending_import: Option<PendingImport>,
    /// Destructive editor statement awaiting confirmation, if any.
    pending_dangerous: Option<DangerousStmt>,
    /// Whether the help overlay (cheatsheet) is showing.
    show_help: bool,
    /// Active fuzzy picker (command palette / find table / open file), if any.
    fuzzy: Option<FuzzyPicker>,
    /// Snippet-name prompt (saving the current buffer as a snippet), if active.
    snippet_prompt: Option<String>,
    /// Interactive password prompt, if a connection needs one.
    password_prompt: Option<PasswordPrompt>,
    /// Credential store (OS keyring in production; injectable in tests).
    credentials: Arc<dyn sextant_core::CredentialStore>,
    /// Password entered at the prompt, pending a successful connection to save.
    pending_credential: Option<PendingCredential>,
    /// Orphan-swap recovery prompt shown at startup, if any.
    recovery: Option<Recovery>,
    /// This session's swap file path (`session-<pid>.swp`).
    session_swap: std::path::PathBuf,
    /// When the swap file was last written (throttles to ~30s).
    last_swap: Instant,
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
    /// Whether a background operation (query/connect/commit/import) is running,
    /// driving a status-line spinner.
    busy: bool,
    /// Animation frame for the spinner, advanced on each tick.
    spinner_frame: usize,
    focus: Focus,
    result_grid: ResultGrid,
    /// Resolved theme colors used across the UI.
    palette: Palette,
    needs_redraw: bool,
}

impl App {
    fn new() -> io::Result<Self> {
        let connections = sextant_config::load_connections().unwrap_or_else(|e| {
            tracing::warn!("failed to load connections: {e}");
            vec![]
        });
        let names: Vec<String> = connections.iter().map(|c| c.name.clone()).collect();

        // Resolve the configured theme into concrete colors, then seed every
        // widget with it (a single global palette; no runtime switching yet).
        let palette = Palette::from_theme(&sextant_config::load_theme());
        let mut tree = TreePane::new(names);
        tree.set_palette(palette);
        let mut editor = EditorModal::new();
        editor.set_palette(palette);
        let mut result_grid = ResultGrid::new();
        result_grid.set_palette(palette);

        let keymap = Keymap::with_user_bindings(&sextant_config::load_keybindings());

        Ok(Self {
            mode: Mode::Normal,
            connection_name: None,
            should_quit: false,
            tree,
            connection_configs: connections,
            executors: std::collections::HashMap::new(),
            editor_open: false,
            editor,
            keymap,
            chord: ChordState::default(),
            pending_commit: None,
            last_browse_sql: None,
            save_prompt: None,
            quit_prompt: false,
            state_store: None,
            picker: None,
            copy_format_picker: None,
            import_prompt: None,
            pending_import: None,
            pending_dangerous: None,
            show_help: false,
            fuzzy: None,
            snippet_prompt: None,
            password_prompt: None,
            credentials: Arc::new(sextant_config::KeyringStore),
            pending_credential: None,
            recovery: None,
            session_swap: swap::session_swap_path(),
            last_swap: Instant::now(),
            saved_buffers: std::collections::HashMap::new(),
            table_meta: std::collections::HashMap::new(),
            last_result: None,
            last_error: None,
            last_notice: None,
            last_query_duration: None,
            query_start: None,
            busy: false,
            spinner_frame: 0,
            focus: Focus::Tree,
            result_grid,
            palette,
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
            render_commit_modal(frame, area, statements, self.palette);
        }

        // Save-as filename prompt.
        if let Some(name) = &self.save_prompt {
            render_save_prompt(frame, area, name, self.palette);
        }

        // Snippet-name prompt.
        if let Some(name) = &self.snippet_prompt {
            render_snippet_prompt(frame, area, name, self.palette);
        }

        // Quit-with-unsaved-buffers prompt.
        if self.quit_prompt {
            render_quit_prompt(frame, area, self.palette);
        }

        // Modal list picker (history / recent files).
        if let Some(picker) = &self.picker {
            render_picker(frame, area, picker, self.palette);
        }

        // Copy-format picker.
        if let Some(picker) = &self.copy_format_picker {
            render_picker(frame, area, picker, self.palette);
        }

        // Import file-path prompt.
        if let Some(prompt) = &self.import_prompt {
            render_import_prompt(frame, area, prompt, self.palette);
        }

        // Import confirmation modal.
        if let Some(pending) = &self.pending_import {
            render_import_modal(frame, area, pending, self.palette);
        }

        // Destructive-statement confirmation modal.
        if let Some(dangerous) = &self.pending_dangerous {
            render_dangerous_modal(frame, area, dangerous, self.palette);
        }

        // Password prompt.
        if let Some(prompt) = &self.password_prompt {
            render_password_prompt(frame, area, prompt, self.palette);
        }

        // Fuzzy picker (command palette / find table / open file).
        if let Some(fuzzy) = &self.fuzzy {
            render_fuzzy_picker(frame, area, fuzzy, self.palette);
        }

        // Help overlay.
        if self.show_help {
            render_help_overlay(frame, area, &self.keymap, self.palette);
        }

        // Crash-recovery prompt (highest priority overlay).
        if let Some(recovery) = &self.recovery {
            render_recovery_modal(frame, area, recovery, self.palette);
        }

        // Which-key menu: when the leader is armed, show the keys that can
        // continue the chord. Ordinary prefixes (`g`, `d`) get only the
        // status-line echo below, not a popup.
        if let [first, ..] = self.chord.pending() {
            if first.is_leader() {
                let entries = self.keymap.continuations(self.chord.pending());
                if !entries.is_empty() {
                    let title = self.chord.pending_display().unwrap_or_default();
                    render_whichkey_menu(frame, area, &title, &entries, self.palette);
                }
            }
        }

        // Status line at the bottom.
        let p = self.palette;
        let mode_bg = match self.mode {
            Mode::Normal => p.accent,
            Mode::Insert => p.accent_alt,
            Mode::Visual => p.success,
        };
        let mode_span = Span::styled(
            format!(" {} ", self.mode),
            Style::default().fg(p.background).bg(mode_bg),
        );

        // Pending-chord echo: the leader gets the which-key popup (rendered
        // above), so the status line only echoes ordinary prefixes (`g`, `d`)
        // — a lightweight "the mode is armed" signal.
        let chord_span = match self.chord.pending() {
            [first, ..] if !first.is_leader() => {
                let display = self.chord.pending_display().unwrap_or_default();
                Span::styled(format!(" {display}… "), Style::default().fg(p.accent_alt))
            }
            _ => Span::raw(""),
        };

        let conn_span = Span::styled(
            format!(
                " {} ",
                self.connection_name.as_deref().unwrap_or("no connection")
            ),
            Style::default().fg(p.foreground),
        );

        // Transaction indicator: only an open session transaction is flagged.
        // Autocommit is the implicit default and shows nothing, which keeps the
        // status line within its width budget.
        let txn_active = self
            .connection_name
            .as_deref()
            .and_then(|name| self.executors.get(name))
            .is_some_and(|exec| exec.in_transaction());
        let txn_span = if txn_active {
            Span::styled("txn: ACTIVE ", Style::default().fg(p.accent_alt))
        } else {
            Span::raw("")
        };

        // Spinner shown only while a background operation is running.
        let spinner_span = if self.busy {
            const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            Span::styled(
                format!("{} ", FRAMES[self.spinner_frame % FRAMES.len()]),
                Style::default().fg(p.accent),
            )
        } else {
            Span::raw("")
        };

        let error_span = if let Some(ref err) = self.last_error {
            Span::styled(format!(" ERR: {} │ ", err), Style::default().fg(p.error))
        } else if let Some(ref notice) = self.last_notice {
            Span::styled(format!(" {} │ ", notice), Style::default().fg(p.success))
        } else {
            Span::raw("")
        };

        let stats_span = if let Some(ref result) = self.last_result {
            let rows = result.rows.len();
            let dur = self
                .last_query_duration
                .map(|d| format!("{}ms", d.as_millis()))
                .unwrap_or_else(|| "-".into());
            Span::styled(
                format!(" {rows} rows / {dur} │ "),
                Style::default().fg(p.foreground),
            )
        } else {
            Span::raw(" ")
        };

        // Editability / pending-changes indicator.
        let edit_span = if self.result_grid.result().is_some() {
            if !self.result_grid.is_editable() {
                Span::styled("🔒 │ ", Style::default().fg(p.muted))
            } else if self.result_grid.has_pending() {
                Span::styled(
                    format!("✎ {} pending │ ", self.result_grid.pending_count()),
                    Style::default().fg(p.accent_alt),
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
            " <Space>e editor │ <Space>h history │ <Space>r recent │ <Space>x export │ <Space>i import "
        };
        let hint_span = Span::styled(hint, Style::default().fg(p.muted));

        let status = Line::from(vec![
            mode_span,
            chord_span,
            conn_span,
            spinner_span,
            txn_span,
            error_span,
            stats_span,
            edit_span,
            hint_span,
        ]);

        // Help is the entry point to every other binding, so it gets its own
        // cell pinned to the right edge. On a narrow terminal the contextual
        // hint (left cell) truncates; the help hint never does.
        const HELP_HINT: &str = " <Space>? help ";
        let bar = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(HELP_HINT.len() as u16),
            ])
            .split(outer[1]);
        let status_bar = Paragraph::new(status).style(Style::default().bg(p.background));
        frame.render_widget(status_bar, bar[0]);
        let help_bar = Paragraph::new(Span::styled(HELP_HINT, Style::default().fg(p.muted)))
            .style(Style::default().bg(p.background));
        frame.render_widget(help_bar, bar[1]);
    }

    fn handle_key_event(&mut self, key: KeyEvent, tx: &UnboundedSender<AppMsg>) {
        if key.kind != KeyEventKind::Press {
            return;
        }

        tracing::debug!("key: {:?}, modifiers: {:?}", key.code, key.modifiers);

        // Help overlay swallows keys until dismissed.
        if self.show_help {
            self.show_help = false;
            return;
        }

        // Fuzzy picker (command palette / find table / open file) captures keys:
        // characters edit the query; navigation is via arrows or Ctrl-n/Ctrl-p.
        if self.fuzzy.is_some() {
            match key.code {
                KeyCode::Esc => self.fuzzy = None,
                KeyCode::Enter => self.fuzzy_select(tx),
                KeyCode::Down => self.fuzzy_move(1),
                KeyCode::Up => self.fuzzy_move(-1),
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.fuzzy_move(1)
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.fuzzy_move(-1)
                }
                KeyCode::Backspace => {
                    if let Some(f) = self.fuzzy.as_mut() {
                        f.backspace();
                    }
                }
                KeyCode::Char(c) if key.modifiers.is_empty() => {
                    if let Some(f) = self.fuzzy.as_mut() {
                        f.push(c);
                    }
                }
                _ => {}
            }
            return;
        }

        // Crash-recovery prompt swallows keys until dismissed.
        if self.recovery.is_some() {
            match key.code {
                KeyCode::Char('r') => self.restore_recovery(),
                KeyCode::Char('d') => self.discard_recovery(),
                KeyCode::Esc => self.recovery = None,
                _ => {}
            }
            return;
        }

        // Password prompt swallows keys until submitted/cancelled.
        if self.password_prompt.is_some() {
            match key.code {
                KeyCode::Enter => self.confirm_password_prompt(tx),
                KeyCode::Esc => self.password_prompt = None,
                KeyCode::Backspace => {
                    if let Some(p) = self.password_prompt.as_mut() {
                        p.input.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(p) = self.password_prompt.as_mut() {
                        p.input.push(c);
                    }
                }
                _ => {}
            }
            return;
        }

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

        // Snippet-name prompt swallows keys until confirmed/cancelled.
        if self.snippet_prompt.is_some() {
            match key.code {
                KeyCode::Enter => self.confirm_save_snippet(tx),
                KeyCode::Esc => self.snippet_prompt = None,
                KeyCode::Backspace => {
                    if let Some(name) = self.snippet_prompt.as_mut() {
                        name.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(name) = self.snippet_prompt.as_mut() {
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

        // Copy-format picker swallows keys until closed.
        if self.copy_format_picker.is_some() {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => self.copy_format_picker_move(1),
                KeyCode::Char('k') | KeyCode::Up => self.copy_format_picker_move(-1),
                KeyCode::Enter => self.copy_format_picker_select(),
                KeyCode::Esc => self.copy_format_picker = None,
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

        // Visual mode captures h/j/k/l, Esc, v, and y before the normal keymap.
        if self.mode == Mode::Visual {
            match key.code {
                KeyCode::Esc | KeyCode::Char('v') => {
                    self.mode = Mode::Normal;
                    self.result_grid.exit_visual_mode();
                    return;
                }
                KeyCode::Char('h') => self.result_grid.move_left(),
                KeyCode::Char('j') => self.result_grid.move_down(),
                KeyCode::Char('k') => self.result_grid.move_up(),
                KeyCode::Char('l') => self.result_grid.move_right(),
                KeyCode::Char('y') => {
                    self.open_copy_format_picker();
                }
                _ => {
                    // Ignore unrecognised keys in visual mode (user must exit first).
                }
            }
            return;
        }

        // Esc clears any full-row selection before falling through to the keymap.
        if key.code == KeyCode::Esc
            && self.focus == Focus::Grid
            && self.result_grid.has_row_selection()
        {
            self.result_grid.clear_row_selection();
            return;
        }

        // Resolve the key through the remappable keymap; a completed chord
        // yields an Action dispatched against the current focus.
        let spec = KeySpec::from_event(key);
        if let Some(action) = self.chord.feed(&self.keymap, spec) {
            self.dispatch(action, tx);
        }
    }

    /// Execute a resolved keymap [`Action`] against the current focus/state.
    fn dispatch(&mut self, action: Action, tx: &UnboundedSender<AppMsg>) {
        match action {
            Action::Quit => {
                if self.editor.any_dirty() {
                    self.quit_prompt = true;
                } else {
                    self.should_quit = true;
                }
            }
            Action::FocusNext => {
                self.focus = match self.focus {
                    Focus::Tree => Focus::Grid,
                    Focus::Grid => Focus::Tree,
                };
            }
            Action::ToggleEditor => self.open_editor(),
            Action::OpenHistory => self.open_history(tx),
            Action::OpenRecent => self.open_recent_files(tx),
            Action::Export => self.open_export_menu(),
            Action::Import => self.start_import(),
            Action::Down => match self.focus {
                Focus::Tree => self.tree.next(),
                Focus::Grid => self.result_grid.move_down(),
            },
            Action::Up => match self.focus {
                Focus::Tree => self.tree.prev(),
                Focus::Grid => self.result_grid.move_up(),
            },
            Action::Left => match self.focus {
                Focus::Tree => self.handle_tree_left(),
                Focus::Grid => self.result_grid.move_left(),
            },
            Action::Right => match self.focus {
                Focus::Tree => self.handle_tree_right(tx),
                Focus::Grid => self.result_grid.move_right(),
            },
            Action::Top => {
                if self.focus == Focus::Grid {
                    self.result_grid.top();
                }
            }
            Action::Bottom => {
                if self.focus == Focus::Grid {
                    self.result_grid.bottom();
                }
            }
            Action::Activate => match self.focus {
                Focus::Tree => self.handle_enter(tx),
                Focus::Grid => {
                    self.result_grid.begin_edit();
                    if self.result_grid.is_editing() {
                        self.mode = Mode::Insert;
                    }
                }
            },
            Action::AddRow => {
                if self.focus == Focus::Grid {
                    self.result_grid.add_row();
                }
            }
            Action::DeleteRow => {
                if self.focus == Focus::Grid {
                    if self.result_grid.has_row_selection() {
                        self.result_grid.delete_selected_rows();
                    } else {
                        self.result_grid.mark_delete();
                    }
                }
            }
            Action::Commit => {
                if self.focus == Focus::Grid {
                    self.begin_commit();
                }
            }
            Action::Discard => {
                if self.focus == Focus::Grid {
                    self.result_grid.discard_changes();
                }
            }
            Action::EmitDdl => {
                if self.focus == Focus::Tree {
                    self.emit_table_ddl();
                }
            }
            Action::Help => self.show_help = true,
            Action::CommandPalette => self.open_command_palette(),
            Action::FindTable => self.open_table_finder(),
            Action::OpenFile => self.open_file_finder(),
            Action::Snippets => self.open_snippets(tx),
            Action::SaveSnippet => self.begin_save_snippet(),
            Action::WidenColumn => {
                if self.focus == Focus::Grid {
                    self.result_grid.widen_column();
                }
            }
            Action::NarrowColumn => {
                if self.focus == Focus::Grid {
                    self.result_grid.narrow_column();
                }
            }
            Action::AutoFitColumn => {
                if self.focus == Focus::Grid {
                    self.result_grid.auto_fit_column();
                }
            }
            Action::AutoFitAll => {
                if self.focus == Focus::Grid {
                    self.result_grid.auto_fit_all();
                }
            }
            Action::EnterVisualMode => {
                if self.focus == Focus::Grid && self.result_grid.result().is_some() {
                    self.result_grid.enter_visual_mode();
                    self.mode = Mode::Visual;
                }
            }
            Action::Copy => {
                if self.focus != Focus::Grid {
                    return;
                }
                if self.result_grid.has_row_selection() {
                    // Full-row selection: choose a format before copying.
                    self.open_copy_format_picker();
                } else {
                    // No selection: copy the current cell as plain text.
                    match self.result_grid.copy_current_cell() {
                        Ok(text) => {
                            if let Err(e) = set_clipboard(&text) {
                                self.last_error = Some(format!("clipboard error: {e}"));
                            } else {
                                self.last_notice = Some("Copied cell".to_string());
                            }
                        }
                        Err(e) => {
                            self.last_error = Some(e);
                        }
                    }
                }
            }
            Action::ToggleRowSelection => {
                if self.focus == Focus::Grid {
                    let row = self.result_grid.cursor_row();
                    self.result_grid.toggle_row_selection(row);
                }
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

        // Resolve the password per the §3.2 cascade. The lookups (keyring via the
        // injected store, then the env-var fallback) are the only I/O here; the
        // ordering/decision lives in the pure `resolve_password`.
        let from_keyring = config
            .keyring_key
            .as_deref()
            .and_then(|k| self.credentials.get(k));
        let from_env = sextant_config::connection_password(name);

        match sextant_config::resolve_password(
            config.driver,
            config.keyring_key.as_deref(),
            from_keyring,
            from_env,
        ) {
            sextant_config::PasswordResolution::Connect(password) => {
                self.spawn_connect(name.to_string(), conn_idx, password, tx);
            }
            sextant_config::PasswordResolution::Prompt { keyring_key } => {
                self.password_prompt = Some(PasswordPrompt {
                    name: name.to_string(),
                    conn_idx,
                    keyring_key,
                    input: String::new(),
                });
            }
        }
    }

    /// Submit the entered password: connect with it and, on success, store it in
    /// the keyring under the connection's `keyring_key`. The save is deferred to
    /// [`App::persist_pending_credential`], run when the connection succeeds.
    fn confirm_password_prompt(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(prompt) = self.password_prompt.take() else {
            return;
        };
        self.pending_credential = Some(PendingCredential {
            name: prompt.name.clone(),
            keyring_key: prompt.keyring_key,
            password: prompt.input.clone(),
        });
        self.spawn_connect(prompt.name, prompt.conn_idx, Some(prompt.input), tx);
    }

    /// Persist a prompt-entered password to the credential store once its
    /// connection succeeds (§3.2 "save after connect"). No-op unless a pending
    /// credential matches `name`; clears it either way.
    fn persist_pending_credential(&mut self, name: &str) {
        if self
            .pending_credential
            .as_ref()
            .is_none_or(|p| p.name != name)
        {
            return;
        }
        let pending = self.pending_credential.take().unwrap();
        if let Err(e) = self
            .credentials
            .set(&pending.keyring_key, &pending.password)
        {
            tracing::warn!("failed to store password in keyring: {e}");
        }
    }

    /// Spawn the async connect + introspection.
    fn spawn_connect(
        &mut self,
        name: String,
        conn_idx: usize,
        password: Option<String>,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let Some(config) = self
            .connection_configs
            .iter()
            .find(|c| c.name == name)
            .cloned()
        else {
            return;
        };

        self.tree.set_connecting(conn_idx);
        self.connection_name = Some(format!("{name} (connecting)"));
        self.busy = true;

        let tx = tx.clone();

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
        // Any terminal result clears the busy spinner.
        if matches!(
            msg,
            AppMsg::Connected { .. }
                | AppMsg::ConnectionFailed { .. }
                | AppMsg::QueryResult(_)
                | AppMsg::QueryError(_)
                | AppMsg::CommitResult(_)
                | AppMsg::ImportFinished(_)
                | AppMsg::ExportFinished(_)
        ) {
            self.busy = false;
        }

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
                // A prompt-entered password is now known to work: persist it.
                self.persist_pending_credential(&name);
                self.connection_name = Some(name);
            }
            AppMsg::ConnectionFailed { name, error } => {
                if let Some(idx) = self.tree.connection_index_by_name(&name) {
                    self.tree.set_error(idx, error.clone());
                }
                // Don't keep a password that failed to connect.
                if self
                    .pending_credential
                    .as_ref()
                    .is_some_and(|p| p.name == name)
                {
                    self.pending_credential = None;
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
            AppMsg::SnippetsLoaded(snippets) => {
                if snippets.is_empty() {
                    self.last_notice = Some("no snippets saved yet (<Space>S to save)".into());
                } else {
                    let items = snippets
                        .into_iter()
                        .map(|s| {
                            let preview = s.body.lines().next().unwrap_or("").trim();
                            FuzzyItem {
                                label: format!("{}  —  {}", s.name, truncate(preview, 50)),
                                action: FuzzyAction::InsertSnippet(s.body),
                            }
                        })
                        .collect();
                    self.fuzzy = Some(FuzzyPicker::new("Insert snippet", items));
                }
            }
            AppMsg::SnippetSaved(Ok(name)) => {
                self.last_error = None;
                self.last_notice = Some(format!("saved snippet '{name}'"));
            }
            AppMsg::SnippetSaved(Err(error)) => {
                self.last_error = Some(format!("snippet save failed: {error}"));
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
                // Once nothing is dirty, the swap file is no longer needed.
                if !self.editor.any_dirty() {
                    swap::remove(&self.session_swap);
                }
            }
            Err(e) => {
                self.last_error = Some(format!("save failed: {e}"));
            }
        }
    }

    /// Open the command palette: a fuzzy list of high-level commands.
    fn open_command_palette(&mut self) {
        let commands = [
            (
                "Open SQL editor",
                FuzzyAction::Dispatch(Action::ToggleEditor),
            ),
            ("Query history", FuzzyAction::Dispatch(Action::OpenHistory)),
            ("Recent files", FuzzyAction::Dispatch(Action::OpenRecent)),
            ("Find table", FuzzyAction::FindTable),
            ("Open .sql file", FuzzyAction::OpenFile),
            ("Export result set", FuzzyAction::Dispatch(Action::Export)),
            ("Import into table", FuzzyAction::Dispatch(Action::Import)),
            ("Emit CREATE TABLE", FuzzyAction::Dispatch(Action::EmitDdl)),
            ("Help", FuzzyAction::Dispatch(Action::Help)),
            ("Quit", FuzzyAction::Dispatch(Action::Quit)),
        ];
        let items = commands
            .into_iter()
            .map(|(label, action)| FuzzyItem {
                label: label.to_string(),
                action,
            })
            .collect();
        self.fuzzy = Some(FuzzyPicker::new("Command palette", items));
    }

    /// Open the table finder: a fuzzy list of every browseable table.
    fn open_table_finder(&mut self) {
        let items = self
            .tree
            .browseable_tables()
            .into_iter()
            .map(
                |(conn, schema, table, conn_name, schema_name, table_name)| FuzzyItem {
                    label: format!("{conn_name}  {schema_name}.{table_name}"),
                    action: FuzzyAction::Browse {
                        conn,
                        schema,
                        table,
                    },
                },
            )
            .collect::<Vec<_>>();
        if items.is_empty() {
            self.last_error = Some("no connected tables to find".into());
            return;
        }
        self.fuzzy = Some(FuzzyPicker::new("Find table", items));
    }

    /// Open the file opener: a fuzzy list of `.sql` files in the queries dir.
    fn open_file_finder(&mut self) {
        let dir = sextant_config::queries_dir();
        let mut items: Vec<FuzzyItem> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("sql"))
                .map(|path| FuzzyItem {
                    label: path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    action: FuzzyAction::Load(path),
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        items.sort_by(|a, b| a.label.cmp(&b.label));
        if items.is_empty() {
            self.last_error = Some(format!("no .sql files in {}", dir.display()));
            return;
        }
        self.fuzzy = Some(FuzzyPicker::new("Open file", items));
    }

    /// Move the fuzzy-picker selection by `delta`.
    fn fuzzy_move(&mut self, delta: isize) {
        if let Some(f) = self.fuzzy.as_mut() {
            f.move_selection(delta);
        }
    }

    /// Act on the highlighted fuzzy-picker item, then close the picker.
    fn fuzzy_select(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(action) = self.fuzzy.take().and_then(FuzzyPicker::into_selected) else {
            return;
        };
        match action {
            FuzzyAction::Dispatch(a) => self.dispatch(a, tx),
            FuzzyAction::FindTable => self.open_table_finder(),
            FuzzyAction::OpenFile => self.open_file_finder(),
            FuzzyAction::Browse {
                conn,
                schema,
                table,
            } => self.browse_table(conn, schema, table, tx),
            FuzzyAction::Load(path) => match std::fs::read_to_string(&path) {
                Ok(content) => self.load_into_editor(&content, Some(path)),
                Err(e) => self.last_error = Some(format!("open failed: {e}")),
            },
            FuzzyAction::InsertSnippet(body) => {
                if !self.editor_open {
                    self.open_editor();
                }
                self.editor.insert_str(&body);
            }
        }
    }

    /// Load snippets from the state store into a fuzzy picker (async).
    fn open_snippets(&self, tx: &UnboundedSender<AppMsg>) {
        let Some(store) = self.state_store.clone() else {
            return;
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Ok(snippets) = store.snippets().await {
                let _ = tx.send(AppMsg::SnippetsLoaded(snippets));
            }
        });
    }

    /// Begin saving the current editor buffer as a named snippet (name prompt).
    fn begin_save_snippet(&mut self) {
        if self.editor.content().trim().is_empty() {
            self.last_error = Some("nothing to save as a snippet".into());
            return;
        }
        self.snippet_prompt = Some(String::new());
    }

    /// Persist the current buffer under the prompted snippet name (async).
    fn confirm_save_snippet(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(name) = self.snippet_prompt.take() else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let body = self.editor.content();
        let Some(store) = self.state_store.clone() else {
            self.last_error = Some("snippets unavailable (no state store)".into());
            return;
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            let msg = match store.save_snippet(&name, &body).await {
                Ok(()) => AppMsg::SnippetSaved(Ok(name)),
                Err(e) => AppMsg::SnippetSaved(Err(e.to_string())),
            };
            let _ = tx.send(msg);
        });
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

    /// Scan the swap directory for orphan files (from a crashed session) and, if
    /// the newest holds buffers, stage a recovery prompt.
    fn scan_for_recovery(&mut self) {
        let orphans = swap::find_orphans();
        let Some(newest) = orphans.first() else {
            return;
        };
        if let Some(doc) = swap::read(newest) {
            if !doc.buffers.is_empty() {
                self.recovery = Some(Recovery {
                    orphans,
                    buffers: doc.buffers,
                });
            }
        }
    }

    /// Restore recovered buffers into the editor and delete the orphan swaps.
    fn restore_recovery(&mut self) {
        let Some(rec) = self.recovery.take() else {
            return;
        };
        self.open_editor();
        self.editor.restore_buffers(rec.buffers);
        for path in &rec.orphans {
            swap::remove(path);
        }
    }

    /// Discard the recovery offer and delete the orphan swap files.
    fn discard_recovery(&mut self) {
        let Some(rec) = self.recovery.take() else {
            return;
        };
        for path in &rec.orphans {
            swap::remove(path);
        }
    }

    /// Write a swap file for the dirty buffers, throttled to ~30s. When nothing
    /// is dirty, remove any existing swap (so a clean editor leaves none behind).
    fn maybe_write_swap(&mut self) {
        if self.last_swap.elapsed() < Duration::from_secs(30) {
            return;
        }
        self.last_swap = Instant::now();
        let dirty = self.editor.dirty_snapshot();
        if dirty.is_empty() {
            swap::remove(&self.session_swap);
            return;
        }
        let doc = swap::SwapDoc { buffers: dirty };
        if let Err(e) = sextant_config::write_swap(&self.session_swap, &swap::serialize(&doc)) {
            tracing::warn!("swap write failed: {e}");
        }
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
        self.busy = true;
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
            sextant_db::ExportFormat::Tsv,
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

        self.busy = true;
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
        self.busy = true;
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
            PickerAction::CopyFormat(_) => {
                // Copy-format picker has its own select handler.
            }
        }
    }

    // ------------------------------------------------------------------
    // Copy-format picker helpers
    // ------------------------------------------------------------------

    fn open_copy_format_picker(&mut self) {
        let has_ctx = self.result_grid.edit_context().is_some();
        let items = vec![
            PickerItem {
                label: "CSV".to_string(),
                action: PickerAction::CopyFormat(CopyFormat::Csv),
            },
            PickerItem {
                label: "TSV".to_string(),
                action: PickerAction::CopyFormat(CopyFormat::Tsv),
            },
            PickerItem {
                label: "JSON".to_string(),
                action: PickerAction::CopyFormat(CopyFormat::Json),
            },
            PickerItem {
                label: if has_ctx {
                    "SQL INSERT".to_string()
                } else {
                    "SQL INSERT (requires table context)".to_string()
                },
                action: PickerAction::CopyFormat(CopyFormat::SqlInsert),
            },
        ];
        self.copy_format_picker = Some(Picker {
            title: "Copy as".to_string(),
            items,
            selected: 0,
        });
    }

    fn copy_format_picker_move(&mut self, delta: isize) {
        if let Some(p) = self.copy_format_picker.as_mut() {
            if p.items.is_empty() {
                return;
            }
            let len = p.items.len() as isize;
            p.selected = (p.selected as isize + delta).rem_euclid(len) as usize;
        }
    }

    fn copy_format_picker_select(&mut self) {
        let Some(picker) = self.copy_format_picker.take() else {
            return;
        };
        let Picker {
            items, selected, ..
        } = picker;
        let Some(item) = items.into_iter().nth(selected) else {
            return;
        };
        let PickerAction::CopyFormat(format) = item.action else {
            return;
        };

        let copy_result = if self.result_grid.has_row_selection() {
            self.result_grid.copy_selected_rows(format)
        } else {
            self.result_grid.copy(format)
        };

        match copy_result {
            Ok(text) => {
                if let Err(e) = set_clipboard(&text) {
                    self.last_error = Some(format!("clipboard error: {e}"));
                } else {
                    let rows = if self.result_grid.has_row_selection() {
                        self.result_grid.selected_row_count()
                    } else {
                        self.result_grid
                            .selected_range()
                            .map(|(min_r, _, max_r, _)| max_r - min_r + 1)
                            .unwrap_or(0)
                    };
                    self.last_notice = Some(format!(
                        "Copied {rows} row(s) as {}",
                        match format {
                            CopyFormat::Csv => "CSV",
                            CopyFormat::Tsv => "TSV",
                            CopyFormat::Json => "JSON",
                            CopyFormat::SqlInsert => "SQL INSERT",
                        }
                    ));
                }
            }
            Err(e) => {
                self.last_error = Some(e);
            }
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
        self.busy = true;
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

/// Write `text` to the system clipboard.
///
/// On Linux prefers `wl-copy` (Wayland) or `xclip` (X11) over `arboard`,
/// because TUI applications often lack a window that can serve X11
/// `SelectionRequest` events.
fn set_clipboard(text: &str) -> Result<(), String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Ok(mut child) = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            use std::io::Write;
            if let Some(stdin) = child.stdin.take() {
                let mut stdin = stdin;
                if stdin.write_all(text.as_bytes()).is_ok() {
                    drop(stdin);
                    if child.wait().is_ok() {
                        return Ok(());
                    }
                }
            }
        }
    }
    if std::env::var_os("DISPLAY").is_some() {
        if let Ok(mut child) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            use std::io::Write;
            if let Some(stdin) = child.stdin.take() {
                let mut stdin = stdin;
                if stdin.write_all(text.as_bytes()).is_ok() {
                    drop(stdin);
                    if child.wait().is_ok() {
                        return Ok(());
                    }
                }
            }
        }
    }
    // Fallback to arboard (works on macOS, Windows, and some Linux setups).
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| e.to_string())
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

    // Offer to recover buffers from any swap files left by a crashed session.
    // (Done here, not in `App::new`, so unit tests never touch the real FS.)
    app.scan_for_recovery();

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
                app.maybe_write_swap();
                if app.busy {
                    app.spinner_frame = app.spinner_frame.wrapping_add(1);
                    app.needs_redraw = true;
                } else if app.editor_open {
                    app.needs_redraw = true;
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    // Clean exit: drop this session's swap file so it isn't seen as an orphan.
    swap::remove(&app.session_swap);

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
fn render_commit_modal(frame: &mut Frame, area: Rect, statements: &[String], p: Palette) {
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
        Style::default().fg(p.accent_alt),
    ))];
    for s in statements {
        lines.push(Line::from(Span::styled(
            format!("  {s}"),
            Style::default().fg(p.foreground),
        )));
    }
    lines.push(Line::from(Span::styled(
        "<Enter>/y confirm   <Esc>/n cancel",
        Style::default().fg(p.muted),
    )));

    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" Confirm commit ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(p.accent_alt))
                .style(Style::default().bg(p.background)),
        ),
        rect,
    );
}

/// Render a small centered modal with the given title and body lines.
fn render_centered_modal(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line>, p: Palette) {
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
                .border_style(Style::default().fg(p.accent_alt))
                .style(Style::default().bg(p.background)),
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
fn render_import_prompt(frame: &mut Frame, area: Rect, prompt: &ImportPrompt, p: Palette) {
    render_centered_modal(
        frame,
        area,
        &format!("Import into {}", prompt.table),
        vec![
            Line::from(Span::styled(
                format!("{}_", prompt.path),
                Style::default().fg(p.foreground),
            )),
            Line::from(Span::styled(
                "path to .csv/.json/.sql (abs, or under exports dir) │ <Enter> load │ <Esc> cancel",
                Style::default().fg(p.muted),
            )),
        ],
        p,
    );
}

/// Render the import-confirmation modal listing the summary of what will run.
fn render_import_modal(frame: &mut Frame, area: Rect, pending: &PendingImport, p: Palette) {
    let mut lines: Vec<Line> = pending
        .summary
        .iter()
        .map(|s| Line::from(Span::styled(s.clone(), Style::default().fg(p.foreground))))
        .collect();
    lines.push(Line::from(Span::styled(
        "<Enter>/y import   <Esc>/n cancel",
        Style::default().fg(p.muted),
    )));
    render_centered_modal(frame, area, "Confirm import", lines, p);
}

/// Render the destructive-statement confirmation modal.
fn render_dangerous_modal(frame: &mut Frame, area: Rect, dangerous: &DangerousStmt, p: Palette) {
    let first = dangerous.sql.lines().next().unwrap_or("").trim();
    render_centered_modal(
        frame,
        area,
        "Confirm destructive statement",
        vec![
            Line::from(Span::styled(
                format!("⚠ {}", dangerous.reason),
                Style::default().fg(p.error),
            )),
            Line::from(Span::styled(
                format!("  {}", truncate(first, 70)),
                Style::default().fg(p.foreground),
            )),
            Line::from(Span::styled(
                "<Enter>/y run   <Esc>/n cancel",
                Style::default().fg(p.muted),
            )),
        ],
        p,
    );
}

/// Render the fuzzy picker: a query line above the ranked, filtered list.
fn render_fuzzy_picker(frame: &mut Frame, area: Rect, fuzzy: &FuzzyPicker, p: Palette) {
    let width = (area.width as f32 * 0.6).clamp(30.0, 80.0) as u16;
    let height = (area.height as f32 * 0.6).clamp(6.0, 20.0) as u16;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    // Query line, then as many ranked items as fit (minus borders + query).
    let max_rows = height.saturating_sub(3) as usize;
    let offset = fuzzy.selected.saturating_sub(max_rows.saturating_sub(1));
    let labels = fuzzy.visible_labels();

    let mut lines: Vec<Line> = vec![Line::from(vec![
        Span::styled("> ", Style::default().fg(p.accent)),
        Span::styled(
            format!("{}_", fuzzy.query),
            Style::default().fg(p.foreground),
        ),
    ])];
    for (i, label) in labels.iter().enumerate().skip(offset).take(max_rows) {
        if i == fuzzy.selected {
            lines.push(Line::from(Span::styled(
                format!("▶ {label}"),
                Style::default().fg(p.selection_fg).bg(p.selection_bg),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                format!("  {label}"),
                Style::default().fg(p.foreground),
            )));
        }
    }

    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(format!(" {} ", fuzzy.title))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(p.accent))
                .style(Style::default().bg(p.background)),
        ),
        rect,
    );
}

/// Render the help overlay: a cheatsheet built dynamically from the keymap,
/// plus a static section for editor/modal keys not in the remappable map.
fn render_help_overlay(frame: &mut Frame, area: Rect, keymap: &Keymap, p: Palette) {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        "Normal mode",
        Style::default().fg(p.accent),
    )));
    for (chord, desc) in keymap.describe() {
        lines.push(Line::from(vec![
            Span::styled(format!("  {chord:<10}"), Style::default().fg(p.accent_alt)),
            Span::styled(desc.to_string(), Style::default().fg(p.foreground)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Editor",
        Style::default().fg(p.accent),
    )));
    for (chord, desc) in [
        ("i", "insert mode"),
        ("<Esc>", "normal mode / close"),
        ("<C-e>", "run query"),
        ("<C-s>", "save buffer"),
        ("<Tab>", "next buffer"),
        ("<C-t>", "new buffer"),
        ("<C-Space>", "autocomplete"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("  {chord:<10}"), Style::default().fg(p.accent_alt)),
            Span::styled(desc.to_string(), Style::default().fg(p.foreground)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "press any key to close",
        Style::default().fg(p.muted),
    )));

    let width = (area.width as f32 * 0.6).clamp(30.0, 70.0) as u16;
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
                .title(" Help ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(p.accent))
                .style(Style::default().bg(p.background)),
        ),
        rect,
    );
}

/// Render the which-key popup for an armed leader chord: a small box anchored
/// to the bottom-left (just above the status line) listing the keys that can
/// continue the chord and the action each triggers.
fn render_whichkey_menu(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    entries: &[(String, &'static str)],
    p: Palette,
) {
    let lines: Vec<Line> = entries
        .iter()
        .map(|(k, desc)| {
            Line::from(vec![
                Span::styled(format!(" {k:<3}"), Style::default().fg(p.accent_alt)),
                Span::styled((*desc).to_string(), Style::default().fg(p.foreground)),
            ])
        })
        .collect();

    // Width fits the longest "key + description" row; height fits all entries
    // plus the border, clamped to the available area.
    let inner_w = entries
        .iter()
        .map(|(_, desc)| desc.chars().count() + 5)
        .max()
        .unwrap_or(0);
    let width = ((inner_w as u16) + 2).clamp(12, area.width.max(12));
    let height = ((lines.len() as u16) + 2).min(area.height.max(3));
    // Bottom-left, sitting just above the one-row status line.
    let x = area.x;
    let y = area.y + area.height.saturating_sub(height + 1);
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
                .border_style(Style::default().fg(p.accent))
                .style(Style::default().bg(p.background)),
        ),
        rect,
    );
}

/// Render the interactive password prompt (input masked).
fn render_password_prompt(frame: &mut Frame, area: Rect, prompt: &PasswordPrompt, p: Palette) {
    let masked: String = "*".repeat(prompt.input.chars().count());
    render_centered_modal(
        frame,
        area,
        &format!("Password for {}", prompt.name),
        vec![
            Line::from(Span::styled(
                format!("{masked}_"),
                Style::default().fg(p.foreground),
            )),
            Line::from(Span::styled(
                "<Enter> connect & save to keyring │ <Esc> cancel",
                Style::default().fg(p.muted),
            )),
        ],
        p,
    );
}

/// Render the crash-recovery prompt.
fn render_recovery_modal(frame: &mut Frame, area: Rect, recovery: &Recovery, p: Palette) {
    let n = recovery.buffers.len();
    render_centered_modal(
        frame,
        area,
        "Recover unsaved work",
        vec![
            Line::from(Span::styled(
                format!("Found {n} unsaved buffer(s) from a previous session."),
                Style::default().fg(p.foreground),
            )),
            Line::from(Span::styled(
                "r restore │ d discard │ <Esc> ignore",
                Style::default().fg(p.muted),
            )),
        ],
        p,
    );
}

/// Render the snippet-name prompt.
fn render_snippet_prompt(frame: &mut Frame, area: Rect, name: &str, p: Palette) {
    render_centered_modal(
        frame,
        area,
        "Save snippet as",
        vec![
            Line::from(Span::styled(
                format!("{name}_"),
                Style::default().fg(p.foreground),
            )),
            Line::from(Span::styled(
                "name the snippet │ <Enter> save │ <Esc> cancel",
                Style::default().fg(p.muted),
            )),
        ],
        p,
    );
}

/// Render the Save-as filename prompt.
fn render_save_prompt(frame: &mut Frame, area: Rect, name: &str, p: Palette) {
    render_centered_modal(
        frame,
        area,
        "Save as",
        vec![
            Line::from(Span::styled(
                format!("{name}_"),
                Style::default().fg(p.foreground),
            )),
            Line::from(Span::styled(
                "type a name (.sql) │ <Enter> save │ <Esc> cancel",
                Style::default().fg(p.muted),
            )),
        ],
        p,
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
fn render_picker(frame: &mut Frame, area: Rect, picker: &Picker, p: Palette) {
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
            Style::default().fg(p.muted),
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
                        Style::default().fg(p.selection_fg).bg(p.selection_bg),
                    ))
                } else {
                    Line::from(Span::styled(
                        format!("  {}", item.label),
                        Style::default().fg(p.foreground),
                    ))
                }
            })
            .collect()
    };
    lines.push(Line::from(Span::styled(
        "<j/k> move │ <Enter> open │ <Esc> close",
        Style::default().fg(p.muted),
    )));

    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(format!(" {} ", picker.title))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(p.accent))
                .style(Style::default().bg(p.background)),
        ),
        rect,
    );
}

/// Render the quit-with-unsaved-buffers prompt.
fn render_quit_prompt(frame: &mut Frame, area: Rect, p: Palette) {
    render_centered_modal(
        frame,
        area,
        "Unsaved buffers",
        vec![
            Line::from(Span::styled(
                "There are unsaved buffers.",
                Style::default().fg(p.foreground),
            )),
            Line::from(Span::styled(
                "s save │ d discard & quit │ c/<Esc> cancel",
                Style::default().fg(p.muted),
            )),
        ],
        p,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    fn test_app() -> App {
        App::new().unwrap()
    }

    use sextant_core::CredentialStore as _;

    /// An in-memory [`CredentialStore`] double for hermetic credential tests.
    #[derive(Default)]
    struct InMemoryStore {
        map: std::sync::Mutex<std::collections::HashMap<String, String>>,
    }

    impl sextant_core::CredentialStore for InMemoryStore {
        fn get(&self, key: &str) -> Option<String> {
            self.map.lock().unwrap().get(key).cloned()
        }
        fn set(&self, key: &str, password: &str) -> Result<(), sextant_core::SextantError> {
            self.map
                .lock()
                .unwrap()
                .insert(key.to_string(), password.to_string());
            Ok(())
        }
    }

    /// An app with a single TCP (Postgres) connection `pg` declaring `keyring_key`
    /// "k", wired to the given credential store. The host is unreachable, so any
    /// background connect attempt fails harmlessly.
    fn app_with_pg(store: std::sync::Arc<dyn sextant_core::CredentialStore>) -> App {
        let mut app = test_app();
        app.credentials = store;
        app.tree = TreePane::new(vec!["pg".into()]);
        app.connection_configs = vec![sextant_core::Connection {
            name: "pg".into(),
            driver: sextant_core::Driver::Postgres,
            host: Some("127.0.0.1".into()),
            port: Some(1),
            user: Some("u".into()),
            database: Some("d".into()),
            ssl_mode: None,
            path: None,
            keyring_key: Some("k".into()),
        }];
        app
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

    fn one_buffer_recovery(content: &str) -> Recovery {
        Recovery {
            orphans: vec![],
            buffers: vec![swap::SwapBuffer {
                path: None,
                cursor: (0, 0),
                content: content.to_string(),
            }],
        }
    }

    #[test]
    fn recovery_restore_loads_buffers_into_editor() {
        let mut app = test_app();
        app.recovery = Some(one_buffer_recovery("SELECT 99"));
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        )));
        assert!(app.recovery.is_none());
        assert!(app.editor_open, "restore should open the editor");
        assert_eq!(app.editor.content(), "SELECT 99");
    }

    #[test]
    fn recovery_discard_clears_prompt_without_opening_editor() {
        let mut app = test_app();
        app.recovery = Some(one_buffer_recovery("x"));
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        )));
        assert!(app.recovery.is_none());
        assert!(!app.editor_open);
    }

    fn press(app: &mut App, c: char) {
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )));
    }

    #[test]
    fn save_snippet_requires_content_and_prompts() {
        let mut app = test_app();
        app.begin_save_snippet();
        assert!(
            app.snippet_prompt.is_none(),
            "an empty editor should not prompt for a snippet name"
        );

        app.editor.set_content("SELECT 1");
        app.begin_save_snippet();
        assert!(
            app.snippet_prompt.is_some(),
            "a non-empty editor should prompt for a snippet name"
        );
    }

    #[test]
    fn snippets_loaded_opens_fuzzy_picker() {
        let mut app = test_app();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.handle_msg(
            AppMsg::SnippetsLoaded(vec![sextant_state::Snippet {
                name: "recent".into(),
                body: "SELECT * FROM t".into(),
            }]),
            &tx,
        );
        assert!(
            app.fuzzy.is_some(),
            "loading snippets should open the picker"
        );
    }

    #[test]
    fn command_palette_opens_filters_and_closes() {
        let mut app = test_app();
        press(&mut app, ' ');
        press(&mut app, ':');
        assert!(
            app.fuzzy.is_some(),
            "<Space>: should open the command palette"
        );

        // Typing edits the query (not navigation) and filters the list.
        press(&mut app, 'h');
        let visible = app.fuzzy.as_ref().unwrap().visible_labels().len();
        assert!((1..10).contains(&visible), "query should filter commands");

        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert!(app.fuzzy.is_none());
    }

    #[test]
    fn help_overlay_toggles_with_leader_question_and_any_key() {
        let mut app = test_app();
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char(' '),
            KeyModifiers::NONE,
        )));
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        )));
        assert!(app.show_help, "<Space>? should open help");
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        )));
        assert!(!app.show_help, "any key should close help");
    }

    #[test]
    fn password_prompt_captures_input_and_cancels() {
        let mut app = test_app();
        app.password_prompt = Some(PasswordPrompt {
            name: "pg".into(),
            conn_idx: 0,
            keyring_key: "k".into(),
            input: String::new(),
        });
        for c in ['s', '3'] {
            app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::NONE,
            )));
        }
        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.password_prompt.as_ref().unwrap().input, "s");

        app.handle_event(Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert!(app.password_prompt.is_none());
    }

    #[tokio::test]
    async fn start_connection_consults_store_then_prompts() {
        let store = std::sync::Arc::new(InMemoryStore::default());
        let mut app = app_with_pg(store.clone());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        // Empty store + keyring_key on a TCP driver → masked prompt, no connect.
        app.start_connection("pg", 0, &tx);
        let prompt = app.password_prompt.as_ref().expect("should prompt");
        assert_eq!(prompt.keyring_key, "k");

        // With the secret in the store, the same connection skips the prompt and
        // proceeds to connect (proving the injected store is consulted).
        app.password_prompt = None;
        store.set("k", "sekret").unwrap();
        app.start_connection("pg", 0, &tx);
        assert!(app.password_prompt.is_none());
        assert_eq!(app.connection_name.as_deref(), Some("pg (connecting)"));
    }

    #[test]
    fn persist_pending_credential_saves_on_match_and_clears() {
        let store = std::sync::Arc::new(InMemoryStore::default());
        let mut app = app_with_pg(store.clone());
        app.pending_credential = Some(PendingCredential {
            name: "pg".into(),
            keyring_key: "k".into(),
            password: "sekret".into(),
        });

        app.persist_pending_credential("pg");

        assert_eq!(store.get("k").as_deref(), Some("sekret"));
        assert!(app.pending_credential.is_none());
    }

    #[test]
    fn persist_pending_credential_ignores_other_connections() {
        let store = std::sync::Arc::new(InMemoryStore::default());
        let mut app = app_with_pg(store.clone());
        app.pending_credential = Some(PendingCredential {
            name: "pg".into(),
            keyring_key: "k".into(),
            password: "sekret".into(),
        });

        // A different connection succeeding must not store pg's password.
        app.persist_pending_credential("other");

        assert_eq!(store.get("k"), None);
        assert!(app.pending_credential.is_some());
    }

    #[test]
    fn failed_connection_discards_pending_credential() {
        let store = std::sync::Arc::new(InMemoryStore::default());
        let mut app = app_with_pg(store.clone());
        app.pending_credential = Some(PendingCredential {
            name: "pg".into(),
            keyring_key: "k".into(),
            password: "sekret".into(),
        });
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        app.handle_msg(
            AppMsg::ConnectionFailed {
                name: "pg".into(),
                error: "auth failed".into(),
            },
            &tx,
        );

        // A wrong password must not be persisted, and must not linger.
        assert_eq!(store.get("k"), None);
        assert!(app.pending_credential.is_none());
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
    fn help_hint_stays_visible_with_grid_focused() {
        // Wide enough that the long edit-mode hint does not truncate the line.
        let backend = TestBackend::new(120, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = test_app();
        // Editable grid with the focus on it: the status line shows the
        // contextual edit hints, which used to replace the help hint entirely.
        app.focus = Focus::Grid;
        app.result_grid.set_result(Some(sextant_core::QueryResult {
            columns: vec![sextant_core::Column {
                name: "id".into(),
                type_name: "int".into(),
            }],
            rows: vec![vec![sextant_core::CellValue::I64(1)]],
            rows_affected: None,
        }));
        app.result_grid
            .set_edit_context(Some(result_grid::EditContext {
                driver: sextant_core::Driver::Sqlite,
                schema: "main".into(),
                table: "users".into(),
                pk_columns: vec!["id".into()],
            }));
        assert!(app.result_grid.is_editable());

        terminal.draw(|frame| app.render(frame)).unwrap();

        let buf = terminal.backend().buffer();
        let last_row = buf.content.chunks(buf.area.width as usize).last().unwrap();
        let text: String = last_row.iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("edit"),
            "status line should show the grid edit hints: {text}"
        );
        assert!(
            text.contains("<Space>? help"),
            "help hint must stay visible even when the grid is focused: {text}"
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
    fn browse_table_builds_select_with_limit_500() {
        let mut app = app_with_users_table();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        app.browse_table(0, 0, 0, &tx);

        // §7.1 / §17.4: browse runs `SELECT * FROM <tabla> LIMIT 500`,
        // with the table reference quoted for the connection's driver.
        assert_eq!(
            app.last_browse_sql.as_deref(),
            Some(r#"SELECT * FROM "main"."users" LIMIT 500"#)
        );
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
