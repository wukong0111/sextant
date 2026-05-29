//! Read-only result grid backed by `ratatui::widgets::Table`.

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Row, Table},
};
use sextant_core::{CellValue, QueryResult};

/// A read-only grid that displays `QueryResult` rows and columns.
#[derive(Debug, Default)]
pub struct ResultGrid {
    result: Option<QueryResult>,
    cursor_row: usize,
    cursor_col: usize,
}

impl ResultGrid {
    /// Create an empty grid.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the displayed result and reset cursor to the top-left.
    pub fn set_result(&mut self, result: Option<QueryResult>) {
        self.result = result;
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    /// Return a reference to the current result, if any.
    pub fn result(&self) -> &Option<QueryResult> {
        &self.result
    }

    /// Move the cursor one row up, clamped at the first row.
    pub fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
    }

    /// Move the cursor one row down, clamped at the last row.
    pub fn move_down(&mut self) {
        if let Some(ref r) = self.result {
            let max = r.rows.len().saturating_sub(1);
            if self.cursor_row < max {
                self.cursor_row += 1;
            }
        }
    }

    /// Move the cursor one column left, clamped at the first column.
    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
    }

    /// Move the cursor one column right, clamped at the last column.
    pub fn move_right(&mut self) {
        if let Some(ref r) = self.result {
            let max = r.columns.len().saturating_sub(1);
            if self.cursor_col < max {
                self.cursor_col += 1;
            }
        }
    }

    /// Jump to the first row.
    pub fn top(&mut self) {
        self.cursor_row = 0;
    }

    /// Jump to the last row.
    pub fn bottom(&mut self) {
        if let Some(ref r) = self.result {
            self.cursor_row = r.rows.len().saturating_sub(1);
        }
    }

    /// Render the grid (or a placeholder) into `area`.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let Some(ref result) = self.result else {
            let placeholder = Block::default()
                .borders(Borders::NONE)
                .style(Style::default().bg(Color::Black));
            frame.render_widget(placeholder, area);
            return;
        };

        if result.columns.is_empty() || result.rows.is_empty() {
            let text = Line::from(Span::styled(
                "No results",
                Style::default().fg(Color::DarkGray),
            ));
            let para = ratatui::widgets::Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::NONE)
                    .style(Style::default().bg(Color::Black)),
            );
            frame.render_widget(para, area);
            return;
        }

        let widths = compute_column_widths(result);

        let header_cells: Vec<Cell> = result
            .columns
            .iter()
            .enumerate()
            .map(|(i, col)| {
                let style = if i == self.cursor_col {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                Cell::new(col.name.clone()).style(style)
            })
            .collect();
        let header = Row::new(header_cells).style(Style::default().bg(Color::Black));

        let rows: Vec<Row> = result
            .rows
            .iter()
            .enumerate()
            .map(|(row_idx, cells)| {
                let cells: Vec<Cell> = cells
                    .iter()
                    .enumerate()
                    .map(|(col_idx, val)| {
                        let text = cell_value_to_string(val);
                        let style = if row_idx == self.cursor_row && col_idx == self.cursor_col {
                            Style::default().fg(Color::Black).bg(Color::Yellow)
                        } else if row_idx == self.cursor_row {
                            Style::default().bg(Color::DarkGray)
                        } else {
                            Style::default()
                        };
                        Cell::new(text).style(style)
                    })
                    .collect();
                Row::new(cells)
            })
            .collect();

        let constraints: Vec<Constraint> = widths
            .iter()
            .map(|&w| Constraint::Length(w as u16))
            .collect();

        let table = Table::new(rows, constraints).header(header).block(
            Block::default()
                .borders(Borders::NONE)
                .style(Style::default().bg(Color::Black)),
        );

        frame.render_widget(table, area);
    }

    /// Current cursor row (useful for tests).
    #[allow(dead_code)]
    pub fn cursor_row(&self) -> usize {
        self.cursor_row
    }

    /// Current cursor column (useful for tests).
    #[allow(dead_code)]
    pub fn cursor_col(&self) -> usize {
        self.cursor_col
    }
}

fn cell_value_to_string(value: &CellValue) -> String {
    match value {
        CellValue::Null => "NULL".to_string(),
        CellValue::Bool(b) => b.to_string(),
        CellValue::I64(v) => v.to_string(),
        CellValue::F64(v) => v.to_string(),
        CellValue::String(s) => s.clone(),
        CellValue::Bytes(_) => "<binary>".to_string(),
    }
}

/// Compute per-column display widths.
/// Width = max(header_len, max_cell_len) clamped to [3, 40].
fn compute_column_widths(result: &QueryResult) -> Vec<usize> {
    let mut widths: Vec<usize> = result.columns.iter().map(|c| c.name.len()).collect();

    for row in &result.rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                let len = cell_value_to_string(cell).len();
                *w = (*w).max(len);
            }
        }
    }

    widths.iter().map(|&w| w.clamp(3, 40)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use sextant_core::{CellValue, Column};

    fn sample_result() -> QueryResult {
        QueryResult {
            columns: vec![
                Column {
                    name: "id".into(),
                    type_name: "int4".into(),
                },
                Column {
                    name: "name".into(),
                    type_name: "text".into(),
                },
            ],
            rows: vec![
                vec![CellValue::I64(1), CellValue::String("Alice".into())],
                vec![CellValue::I64(2), CellValue::String("Bob".into())],
            ],
            rows_affected: None,
        }
    }

    #[test]
    fn grid_renders_placeholder_when_empty() {
        let backend = TestBackend::new(30, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let grid = ResultGrid::new();

        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        // No "No results" text because result is None; we just render a black block.
        // If we set an empty result explicitly, it should show "No results".
        assert!(text.contains("No results") || text.chars().all(|c| c == ' '));
    }

    #[test]
    fn grid_renders_placeholder_when_no_rows() {
        let backend = TestBackend::new(30, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut grid = ResultGrid::new();
        grid.set_result(Some(QueryResult {
            columns: vec![Column {
                name: "x".into(),
                type_name: "int".into(),
            }],
            rows: vec![],
            rows_affected: None,
        }));

        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("No results"),
            "expected 'No results' placeholder, got: {text}"
        );
    }

    #[test]
    fn grid_renders_rows_and_columns() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));

        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("id"), "header should contain 'id': {text}");
        assert!(
            text.contains("name"),
            "header should contain 'name': {text}"
        );
        assert!(text.contains("Alice"), "row should contain 'Alice': {text}");
        assert!(text.contains("Bob"), "row should contain 'Bob': {text}");
    }

    #[test]
    fn navigation_clamps_at_bounds() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));

        // Already at top
        grid.move_up();
        assert_eq!(grid.cursor_row, 0);

        // Move down twice
        grid.move_down();
        assert_eq!(grid.cursor_row, 1);
        grid.move_down();
        assert_eq!(grid.cursor_row, 1); // clamped

        // Move back up
        grid.move_up();
        assert_eq!(grid.cursor_row, 0);
    }

    #[test]
    fn left_right_clamps_columns() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));

        assert_eq!(grid.cursor_col, 0);
        grid.move_left();
        assert_eq!(grid.cursor_col, 0); // clamped

        grid.move_right();
        assert_eq!(grid.cursor_col, 1);
        grid.move_right();
        assert_eq!(grid.cursor_col, 1); // clamped
    }

    #[test]
    fn top_bottom_works() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));

        grid.bottom();
        assert_eq!(grid.cursor_row, 1);

        grid.top();
        assert_eq!(grid.cursor_row, 0);
    }

    #[test]
    fn column_width_capped_at_40() {
        let long_string = "a".repeat(100);
        let result = QueryResult {
            columns: vec![Column {
                name: "col".into(),
                type_name: "text".into(),
            }],
            rows: vec![vec![CellValue::String(long_string)]],
            rows_affected: None,
        };
        let widths = compute_column_widths(&result);
        assert_eq!(widths, vec![40]);
    }

    #[test]
    fn null_renders_as_null() {
        let result = QueryResult {
            columns: vec![Column {
                name: "x".into(),
                type_name: "text".into(),
            }],
            rows: vec![vec![CellValue::Null]],
            rows_affected: None,
        };
        assert_eq!(cell_value_to_string(&result.rows[0][0]), "NULL");
    }

    #[test]
    fn bytes_renders_as_binary() {
        let result = QueryResult {
            columns: vec![Column {
                name: "data".into(),
                type_name: "bytea".into(),
            }],
            rows: vec![vec![CellValue::Bytes(vec![0xDE, 0xAD])]],
            rows_affected: None,
        };
        assert_eq!(cell_value_to_string(&result.rows[0][0]), "<binary>");
    }
}
