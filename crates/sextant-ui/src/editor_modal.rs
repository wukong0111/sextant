//! Floating SQL editor modal backed by `tui-textarea`.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear},
};
use tui_textarea::TextArea;

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
        }
    }

    /// Render the modal centered over the given full-screen area.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let modal_area = centered_rect(80, 60, area);
        frame.render_widget(Clear, modal_area);
        frame.render_widget(&self.textarea, modal_area);
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
                (super::Mode::Insert, EditorAction::None)
            }
        }
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
}
