//! Floating SQL editor modal backed by `tui-textarea`.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, List, ListItem},
};
use tui_textarea::TextArea;

use crate::autocomplete::{self, SchemaIndex};

/// State of an active autocomplete popup.
struct Completion {
    candidates: Vec<String>,
    selected: usize,
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

/// A floating modal text editor for SQL.
pub struct EditorModal {
    textarea: TextArea<'static>,
    dirty: bool,
    /// Table/column names for autocomplete on the active connection.
    index: SchemaIndex,
    /// Active autocomplete popup, if any.
    completion: Option<Completion>,
}

impl Default for EditorModal {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorModal {
    /// Create a new editor modal with an empty buffer.
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_block(
            Block::default()
                .title(" SQL Editor ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        );
        textarea.set_style(Style::default().fg(Color::White).bg(Color::Black));
        textarea.set_cursor_style(Style::default().fg(Color::Yellow));
        Self {
            textarea,
            dirty: false,
            index: SchemaIndex::new(),
            completion: None,
        }
    }

    /// Provide the table/column names used for autocomplete.
    pub fn set_completion_source(&mut self, index: SchemaIndex) {
        self.index = index;
    }

    /// Render the modal centered over the given full-screen area.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let modal_area = centered_rect(80, 60, area);
        frame.render_widget(Clear, modal_area);
        frame.render_widget(&self.textarea, modal_area);
        self.render_completion(frame, modal_area, area);
    }

    fn render_completion(&self, frame: &mut Frame, modal_area: Rect, area: Rect) {
        let Some(comp) = &self.completion else {
            return;
        };
        if comp.candidates.is_empty() {
            return;
        }

        let (row, col) = self.textarea.cursor();
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

        let items: Vec<ListItem> = comp
            .candidates
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let style = if i == comp.selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(c.clone()).style(style)
            })
            .collect();

        frame.render_widget(Clear, popup);
        frame.render_widget(
            List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .style(Style::default().bg(Color::Black)),
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

        match mode {
            super::Mode::Normal => match key.code {
                KeyCode::Char('i') if key.modifiers.is_empty() => {
                    (super::Mode::Insert, EditorAction::None)
                }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.mark_saved();
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
                            self.textarea.input(input);
                            self.dirty = true;
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
                    self.mark_saved();
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
                self.textarea.input(input);
                self.dirty = true;

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
        let (row, col) = self.textarea.cursor();
        let lines = self.textarea.lines();
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
        for _ in 0..prefix.chars().count() {
            self.textarea.delete_char();
        }
        self.textarea.insert_str(&choice);
        self.dirty = true;
    }

    /// Return the full buffer content as a single string.
    pub fn content(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Replace the buffer content.
    pub fn set_content(&mut self, text: &str) {
        let lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
        self.textarea = TextArea::new(lines);
        // Re-apply styling because `TextArea::new` creates a fresh widget.
        self.textarea.set_block(
            Block::default()
                .title(" SQL Editor ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        );
        self.textarea
            .set_style(Style::default().fg(Color::White).bg(Color::Black));
        self.textarea
            .set_cursor_style(Style::default().fg(Color::Yellow));
        self.dirty = false;
        self.completion = None;
    }

    /// True if the buffer has unsaved changes.
    #[allow(dead_code)] // public API; will be used by status-line dirty indicator in 1.5+
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark the buffer as saved (clears the dirty flag).
    pub fn mark_saved(&mut self) {
        self.dirty = false;
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
        editor.textarea.move_cursor(tui_textarea::CursorMove::End);
        editor.handle_key(key_ctrl(' '), super::super::Mode::Insert);
        let comp = editor.completion.as_ref().expect("popup should open");
        assert_eq!(comp.candidates, vec!["users".to_string()]);
    }

    #[test]
    fn typing_dot_after_table_autocompletes_columns() {
        let mut editor = editor_with_index();
        editor.set_content("SELECT users");
        editor.textarea.move_cursor(tui_textarea::CursorMove::End);
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
        editor.textarea.move_cursor(tui_textarea::CursorMove::End);
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
        editor.textarea.move_cursor(tui_textarea::CursorMove::End);
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
        editor.textarea.move_cursor(tui_textarea::CursorMove::End);
        editor.handle_key(key_ctrl(' '), super::super::Mode::Insert);
        assert!(editor.completion.is_none());
    }
}
