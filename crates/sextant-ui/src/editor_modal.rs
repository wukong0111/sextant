//! Floating SQL editor modal backed by `tui-textarea`.

use std::path::PathBuf;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use tui_textarea::TextArea;

use crate::autocomplete::{self, SchemaIndex};
use crate::palette::Palette;

/// State of an active autocomplete popup.
struct Completion {
    candidates: Vec<String>,
    selected: usize,
}

/// A single editor buffer (tab).
struct Buffer {
    textarea: TextArea<'static>,
    dirty: bool,
    /// File this buffer is bound to (set after the first save).
    path: Option<PathBuf>,
}

impl Buffer {
    fn new(lines: Vec<String>) -> Self {
        Self {
            textarea: styled_textarea(lines),
            dirty: false,
            path: None,
        }
    }
}

/// Build a `TextArea` with the editor's standard block/styling applied.
fn styled_textarea(lines: Vec<String>) -> TextArea<'static> {
    let mut textarea = if lines.is_empty() {
        TextArea::default()
    } else {
        TextArea::new(lines)
    };
    textarea.set_block(
        Block::default()
            .title(" SQL Editor ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    textarea.set_style(Style::default().fg(Color::White).bg(Color::Black));
    textarea.set_cursor_style(Style::default().fg(Color::Yellow));
    textarea
}

/// Actions the editor can request from the host application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorAction {
    /// Nothing to do.
    None,
    /// User pressed `Ctrl+Enter` — execute the current buffer.
    Execute,
    /// User pressed `Ctrl+S` — save the current buffer.
    Save,
    /// User pressed `Esc` in Normal mode — close the modal.
    Close,
}

/// A floating modal text editor for SQL with multiple buffers (tabs).
pub struct EditorModal {
    buffers: Vec<Buffer>,
    active: usize,
    /// Table/column names for autocomplete on the active connection.
    index: SchemaIndex,
    /// Active autocomplete popup, if any.
    completion: Option<Completion>,
    /// Colors for borders, tabs, cursor and the completion popup.
    palette: Palette,
}

impl Default for EditorModal {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorModal {
    /// Create a new editor modal with a single empty buffer.
    pub fn new() -> Self {
        Self {
            buffers: vec![Buffer::new(vec![])],
            active: 0,
            index: SchemaIndex::new(),
            completion: None,
            palette: Palette::default(),
        }
    }

    /// Set the color palette used when rendering.
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    fn active(&self) -> &Buffer {
        &self.buffers[self.active]
    }

    fn active_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active]
    }

    /// Switch to the next buffer (wrapping).
    pub fn next_buffer(&mut self) {
        self.active = (self.active + 1) % self.buffers.len();
        self.completion = None;
    }

    /// Switch to the previous buffer (wrapping).
    pub fn prev_buffer(&mut self) {
        let n = self.buffers.len();
        self.active = (self.active + n - 1) % n;
        self.completion = None;
    }

    /// Open a new empty buffer and focus it.
    pub fn new_buffer(&mut self) {
        self.buffers.push(Buffer::new(vec![]));
        self.active = self.buffers.len() - 1;
        self.completion = None;
    }

    /// Provide the table/column names used for autocomplete.
    pub fn set_completion_source(&mut self, index: SchemaIndex) {
        self.index = index;
    }

    /// Path the active buffer is bound to, if it has been saved.
    pub fn active_path(&self) -> Option<PathBuf> {
        self.active().path.clone()
    }

    /// Bind the active buffer to a file path (after its first save).
    pub fn set_active_path(&mut self, path: PathBuf) {
        self.active_mut().path = Some(path);
    }

    /// True if any buffer has unsaved changes (used for the quit prompt).
    pub fn any_dirty(&self) -> bool {
        self.buffers.iter().any(|b| b.dirty)
    }

    /// Render the modal centered over the given full-screen area.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let modal_area = centered_rect(80, 60, area);
        frame.render_widget(Clear, modal_area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(modal_area);
        let tab_area = chunks[0];
        let editor_area = chunks[1];

        let p = self.palette;

        // Tab bar: one chip per buffer, with `●` for unsaved changes.
        let mut spans = Vec::new();
        for (i, buf) in self.buffers.iter().enumerate() {
            let name = buf
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("{}", i + 1));
            let label = format!(" {}{} ", name, if buf.dirty { "●" } else { "" });
            let style = if i == self.active {
                Style::default().fg(p.selection_fg).bg(p.selection_bg)
            } else {
                Style::default().fg(p.muted)
            };
            spans.push(Span::styled(label, style));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(p.background)),
            tab_area,
        );

        // Apply the theme to the active textarea, then render it.
        let textarea = &mut self.buffers[self.active].textarea;
        textarea.set_block(
            Block::default()
                .title(" SQL Editor ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(p.accent)),
        );
        textarea.set_style(Style::default().fg(p.foreground).bg(p.background));
        textarea.set_cursor_style(Style::default().fg(p.accent_alt));
        frame.render_widget(&self.buffers[self.active].textarea, editor_area);
        self.render_completion(frame, editor_area, area);
    }

    fn render_completion(&self, frame: &mut Frame, editor_area: Rect, area: Rect) {
        let Some(comp) = &self.completion else {
            return;
        };
        if comp.candidates.is_empty() {
            return;
        }

        let (row, col) = self.active().textarea.cursor();
        let modal_area = editor_area;
        let cx = modal_area.x + 1 + col as u16;
        let cy = modal_area.y + 1 + row as u16 + 1;

        let height = (comp.candidates.len() as u16).min(8) + 2;
        let width = comp
            .candidates
            .iter()
            .map(|c| c.chars().count())
            .max()
            .unwrap_or(10)
            .clamp(10, 40) as u16
            + 2;

        let x = cx.min(area.right().saturating_sub(width));
        let y = cy.min(area.bottom().saturating_sub(height));
        let popup = Rect {
            x,
            y,
            width,
            height,
        };

        let p = self.palette;
        let items: Vec<ListItem> = comp
            .candidates
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let style = if i == comp.selected {
                    Style::default().fg(p.selection_fg).bg(p.selection_bg)
                } else {
                    Style::default().fg(p.foreground)
                };
                ListItem::new(c.clone()).style(style)
            })
            .collect();

        frame.render_widget(Clear, popup);
        frame.render_widget(
            List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(p.accent))
                    .style(Style::default().bg(p.background)),
            ),
            popup,
        );
    }

    /// Process a key event and return the new mode + any action.
    ///
    /// When `mode` is `Normal` we interpret vim-like bindings.
    /// When `mode` is `Insert` we forward keys to the textarea.
    pub fn handle_key(
        &mut self,
        key: ratatui::crossterm::event::KeyEvent,
        mode: super::Mode,
    ) -> (super::Mode, EditorAction) {
        use ratatui::crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

        if key.kind != KeyEventKind::Press {
            return (mode, EditorAction::None);
        }

        // Buffer management works in either mode.
        if key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.new_buffer();
            return (mode, EditorAction::None);
        }

        match mode {
            super::Mode::Normal => match key.code {
                KeyCode::Char('i') if key.modifiers.is_empty() => {
                    (super::Mode::Insert, EditorAction::None)
                }
                KeyCode::Tab => {
                    self.next_buffer();
                    (super::Mode::Normal, EditorAction::None)
                }
                KeyCode::BackTab => {
                    self.prev_buffer();
                    (super::Mode::Normal, EditorAction::None)
                }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    (super::Mode::Normal, EditorAction::Save)
                }
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    (super::Mode::Normal, EditorAction::Execute)
                }
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    (super::Mode::Normal, EditorAction::Execute)
                }
                KeyCode::Esc => (super::Mode::Normal, EditorAction::Close),
                _ => (super::Mode::Normal, EditorAction::None),
            },
            super::Mode::Insert => {
                // Manual trigger works whether or not the popup is already open.
                if key.code == KeyCode::Char(' ') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.trigger_completion();
                    return (super::Mode::Insert, EditorAction::None);
                }

                if self.completion.is_some() {
                    match key.code {
                        KeyCode::Esc => {
                            self.completion = None;
                            return (super::Mode::Insert, EditorAction::None);
                        }
                        KeyCode::Up => {
                            self.move_completion(-1);
                            return (super::Mode::Insert, EditorAction::None);
                        }
                        KeyCode::Down => {
                            self.move_completion(1);
                            return (super::Mode::Insert, EditorAction::None);
                        }
                        KeyCode::Tab | KeyCode::Enter if key.modifiers.is_empty() => {
                            self.accept_completion();
                            return (super::Mode::Insert, EditorAction::None);
                        }
                        KeyCode::Char(_) | KeyCode::Backspace
                            if !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            // Type-through: edit then re-filter the popup.
                            let input: tui_textarea::Input = key.into();
                            let buf = self.active_mut();
                            buf.textarea.input(input);
                            buf.dirty = true;
                            self.refresh_completion();
                            return (super::Mode::Insert, EditorAction::None);
                        }
                        _ => {
                            // Any other key dismisses the popup and is handled below.
                            self.completion = None;
                        }
                    }
                }

                if key.code == KeyCode::Esc && key.modifiers.is_empty() {
                    return (super::Mode::Normal, EditorAction::None);
                }

                if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return (super::Mode::Insert, EditorAction::Save);
                }

                if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return (super::Mode::Insert, EditorAction::Execute);
                }

                if key.code == KeyCode::Char('e') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return (super::Mode::Insert, EditorAction::Execute);
                }

                // Forward everything else to the textarea.
                let input: tui_textarea::Input = key.into();
                let buf = self.active_mut();
                buf.textarea.input(input);
                buf.dirty = true;

                // Auto-trigger completion right after a `.`.
                if key.code == KeyCode::Char('.') && key.modifiers.is_empty() {
                    self.trigger_completion();
                }
                (super::Mode::Insert, EditorAction::None)
            }
        }
    }

    /// Text from the start of the buffer up to the cursor.
    fn text_before_cursor(&self) -> String {
        let (row, col) = self.active().textarea.cursor();
        let lines = self.active().textarea.lines();
        let mut out = String::new();
        for line in lines.iter().take(row) {
            out.push_str(line);
            out.push('\n');
        }
        if let Some(line) = lines.get(row) {
            let prefix: String = line.chars().take(col).collect();
            out.push_str(&prefix);
        }
        out
    }

    /// Open (or refresh) the completion popup for the current cursor context.
    fn trigger_completion(&mut self) {
        if self.index.is_empty() {
            return;
        }
        let full = self.content();
        let before = self.text_before_cursor();
        let candidates = autocomplete::complete(&full, &before, &self.index);
        self.completion = if candidates.is_empty() {
            None
        } else {
            Some(Completion {
                candidates,
                selected: 0,
            })
        };
    }

    /// Recompute candidates after a type-through edit; dismiss if none remain.
    fn refresh_completion(&mut self) {
        if self.completion.is_none() {
            return;
        }
        let full = self.content();
        let before = self.text_before_cursor();
        let candidates = autocomplete::complete(&full, &before, &self.index);
        if candidates.is_empty() {
            self.completion = None;
        } else if let Some(comp) = self.completion.as_mut() {
            comp.selected = comp.selected.min(candidates.len() - 1);
            comp.candidates = candidates;
        }
    }

    /// Move the popup selection by `delta`, wrapping around.
    fn move_completion(&mut self, delta: i32) {
        if let Some(comp) = self.completion.as_mut() {
            let len = comp.candidates.len() as i32;
            if len == 0 {
                return;
            }
            comp.selected = (comp.selected as i32 + delta).rem_euclid(len) as usize;
        }
    }

    /// Replace the partial word at the cursor with the selected candidate.
    fn accept_completion(&mut self) {
        let Some(comp) = self.completion.take() else {
            return;
        };
        let Some(choice) = comp.candidates.get(comp.selected).cloned() else {
            return;
        };
        let before = self.text_before_cursor();
        let prefix = autocomplete::current_prefix(&before);
        let buf = self.active_mut();
        for _ in 0..prefix.chars().count() {
            buf.textarea.delete_char();
        }
        buf.textarea.insert_str(&choice);
        buf.dirty = true;
    }

    /// Return the active buffer content as a single string.
    pub fn content(&self) -> String {
        self.active().textarea.lines().join("\n")
    }

    /// Replace the active buffer's content.
    pub fn set_content(&mut self, text: &str) {
        let lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
        let buf = self.active_mut();
        buf.textarea = styled_textarea(lines);
        buf.dirty = false;
        self.completion = None;
    }

    /// True if the active buffer has unsaved changes.
    #[allow(dead_code)] // exercised by tests; surfaced in the tab bar.
    pub fn is_dirty(&self) -> bool {
        self.active().dirty
    }

    /// Mark the active buffer as saved (clears the dirty flag).
    pub fn mark_saved(&mut self) {
        self.active_mut().dirty = false;
    }
}

/// Compute a centered rectangle of `percent_x` × `percent_y` inside `area`.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key_char(c: char) -> KeyEvent {
        KeyEvent::from(KeyCode::Char(c))
    }

    fn key_ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn new_editor_is_not_dirty() {
        let editor = EditorModal::new();
        assert!(!editor.is_dirty());
        assert!(editor.content().is_empty());
    }

    #[test]
    fn normal_mode_i_switches_to_insert() {
        let mut editor = EditorModal::new();
        let (mode, action) = editor.handle_key(key_char('i'), super::super::Mode::Normal);
        assert_eq!(mode, super::super::Mode::Insert);
        assert_eq!(action, EditorAction::None);
    }

    #[test]
    fn normal_mode_esc_closes() {
        let mut editor = EditorModal::new();
        let (mode, action) =
            editor.handle_key(KeyEvent::from(KeyCode::Esc), super::super::Mode::Normal);
        assert_eq!(mode, super::super::Mode::Normal);
        assert_eq!(action, EditorAction::Close);
    }

    #[test]
    fn normal_mode_ctrl_s_saves() {
        let mut editor = EditorModal::new();
        let (mode, action) = editor.handle_key(key_ctrl('s'), super::super::Mode::Normal);
        assert_eq!(mode, super::super::Mode::Normal);
        assert_eq!(action, EditorAction::Save);
    }

    #[test]
    fn insert_mode_esc_returns_to_normal() {
        let mut editor = EditorModal::new();
        let (mode, action) =
            editor.handle_key(KeyEvent::from(KeyCode::Esc), super::super::Mode::Insert);
        assert_eq!(mode, super::super::Mode::Normal);
        assert_eq!(action, EditorAction::None);
    }

    #[test]
    fn typing_in_insert_marks_dirty() {
        let mut editor = EditorModal::new();
        assert!(!editor.is_dirty());
        editor.handle_key(key_char('S'), super::super::Mode::Insert);
        assert!(editor.is_dirty());
        assert_eq!(editor.content(), "S");
    }

    #[test]
    fn set_content_restores_buffer() {
        let mut editor = EditorModal::new();
        editor.set_content("SELECT 1\nFROM users");
        assert_eq!(editor.content(), "SELECT 1\nFROM users");
        assert!(!editor.is_dirty());
    }

    #[test]
    fn save_clears_dirty() {
        let mut editor = EditorModal::new();
        editor.handle_key(key_char('x'), super::super::Mode::Insert);
        assert!(editor.is_dirty());
        editor.mark_saved();
        assert!(!editor.is_dirty());
    }

    #[test]
    fn insert_mode_ctrl_enter_executes() {
        let mut editor = EditorModal::new();
        let (mode, action) = editor.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
            super::super::Mode::Insert,
        );
        assert_eq!(mode, super::super::Mode::Insert);
        assert_eq!(action, EditorAction::Execute);
    }

    #[test]
    fn insert_mode_ctrl_s_saves() {
        let mut editor = EditorModal::new();
        let (mode, action) = editor.handle_key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            super::super::Mode::Insert,
        );
        assert_eq!(mode, super::super::Mode::Insert);
        assert_eq!(action, EditorAction::Save);
    }

    #[test]
    fn normal_mode_ctrl_e_executes() {
        let mut editor = EditorModal::new();
        let (mode, action) = editor.handle_key(
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
            super::super::Mode::Normal,
        );
        assert_eq!(mode, super::super::Mode::Normal);
        assert_eq!(action, EditorAction::Execute);
    }

    fn editor_with_index() -> EditorModal {
        let mut editor = EditorModal::new();
        let mut index = SchemaIndex::new();
        index.add_table("users".into(), vec!["id".into(), "name".into()]);
        editor.set_completion_source(index);
        editor
    }

    #[test]
    fn ctrl_space_triggers_table_completion() {
        let mut editor = editor_with_index();
        editor.set_content("SELECT * FROM ");
        // Move cursor to end of buffer.
        editor.buffers[editor.active]
            .textarea
            .move_cursor(tui_textarea::CursorMove::End);
        editor.handle_key(key_ctrl(' '), super::super::Mode::Insert);
        let comp = editor.completion.as_ref().expect("popup should open");
        assert_eq!(comp.candidates, vec!["users".to_string()]);
    }

    #[test]
    fn typing_dot_after_table_autocompletes_columns() {
        let mut editor = editor_with_index();
        editor.set_content("SELECT users");
        editor.buffers[editor.active]
            .textarea
            .move_cursor(tui_textarea::CursorMove::End);
        // Typing '.' should auto-open the column popup for `users`.
        editor.handle_key(key_char('.'), super::super::Mode::Insert);
        let comp = editor
            .completion
            .as_ref()
            .expect("popup should open on dot");
        assert_eq!(comp.candidates, vec!["id".to_string(), "name".to_string()]);
    }

    #[test]
    fn enter_accepts_completion_and_replaces_prefix() {
        let mut editor = editor_with_index();
        editor.set_content("SELECT * FROM us");
        editor.buffers[editor.active]
            .textarea
            .move_cursor(tui_textarea::CursorMove::End);
        editor.handle_key(key_ctrl(' '), super::super::Mode::Insert);
        assert!(editor.completion.is_some());

        let (_, action) =
            editor.handle_key(KeyEvent::from(KeyCode::Enter), super::super::Mode::Insert);
        assert_eq!(action, EditorAction::None);
        assert!(editor.completion.is_none());
        assert_eq!(editor.content(), "SELECT * FROM users");
    }

    #[test]
    fn esc_dismisses_popup_without_leaving_insert() {
        let mut editor = editor_with_index();
        editor.set_content("SELECT * FROM ");
        editor.buffers[editor.active]
            .textarea
            .move_cursor(tui_textarea::CursorMove::End);
        editor.handle_key(key_ctrl(' '), super::super::Mode::Insert);
        assert!(editor.completion.is_some());

        let (mode, _) = editor.handle_key(KeyEvent::from(KeyCode::Esc), super::super::Mode::Insert);
        assert_eq!(mode, super::super::Mode::Insert);
        assert!(editor.completion.is_none());
    }

    #[test]
    fn completion_noop_without_index() {
        let mut editor = EditorModal::new();
        editor.set_content("SELECT * FROM ");
        editor.buffers[editor.active]
            .textarea
            .move_cursor(tui_textarea::CursorMove::End);
        editor.handle_key(key_ctrl(' '), super::super::Mode::Insert);
        assert!(editor.completion.is_none());
    }

    #[test]
    fn ctrl_t_opens_new_buffer() {
        let mut editor = EditorModal::new();
        editor.set_content("SELECT 1");
        assert_eq!(editor.buffers.len(), 1);

        editor.handle_key(key_ctrl('t'), super::super::Mode::Normal);
        assert_eq!(editor.buffers.len(), 2);
        assert_eq!(editor.active, 1);
        assert!(editor.content().is_empty());
    }

    #[test]
    fn tab_cycles_buffers_in_normal_mode() {
        let mut editor = EditorModal::new();
        editor.set_content("first");
        editor.new_buffer();
        editor.set_content("second");
        assert_eq!(editor.content(), "second");

        editor.handle_key(KeyEvent::from(KeyCode::Tab), super::super::Mode::Normal);
        assert_eq!(editor.active, 0);
        assert_eq!(editor.content(), "first");

        editor.handle_key(KeyEvent::from(KeyCode::BackTab), super::super::Mode::Normal);
        assert_eq!(editor.active, 1);
        assert_eq!(editor.content(), "second");
    }

    #[test]
    fn dirty_is_tracked_per_buffer() {
        let mut editor = EditorModal::new();
        editor.handle_key(key_char('x'), super::super::Mode::Insert);
        assert!(editor.buffers[0].dirty);

        editor.new_buffer();
        assert!(!editor.is_dirty());
        assert!(editor.buffers[0].dirty);
    }
}
