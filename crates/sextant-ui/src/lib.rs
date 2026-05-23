use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame, Terminal,
};
use std::io::{self, stdout, Stdout};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
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
}

impl Default for App {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            connection_name: None,
            should_quit: false,
        }
    }
}

impl App {
    /// Render the application into the given frame.
    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();

        // Main empty area
        let main = Paragraph::new("").block(
            Block::default()
                .borders(Borders::NONE)
                .style(Style::default().bg(Color::Black)),
        );
        frame.render_widget(main, area);

        // Status line at the bottom
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);

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

        let hint_span = Span::styled(" <C-q> quit ", Style::default().fg(Color::DarkGray));

        let status = Line::from(vec![mode_span, conn_span, hint_span]);
        let status_bar = Paragraph::new(status).style(Style::default().bg(Color::Black));
        frame.render_widget(status_bar, chunks[1]);
    }

    fn draw(&self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
        terminal.draw(|frame| self.render(frame))?;
        Ok(())
    }

    fn handle_event(&mut self, event: Event) {
        if let Event::Key(key) = event {
            if key.kind != KeyEventKind::Press {
                return;
            }

            match key.code {
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.should_quit = true;
                }
                _ => {}
            }
        }
    }
}

/// Run the TUI event loop until the user quits.
pub fn run() -> io::Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = App::default();

    let result = (|| -> io::Result<()> {
        loop {
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

    #[test]
    fn renders_status_line() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = App::default();

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
    fn ctrl_q_sets_should_quit() {
        let mut app = App::default();
        assert!(!app.should_quit);

        app.handle_event(Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
        )));

        assert!(app.should_quit);
    }
}
