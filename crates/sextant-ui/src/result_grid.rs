//! Result grid backed by `ratatui::widgets::Table`, with inline editing.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Row, Table},
};
use sextant_core::{CellValue, Driver, QueryResult};

use crate::palette::Palette;

/// Identifies the table a grid was populated from, enabling edits.
///
/// A grid is editable only when it has an `EditContext` **and** a non-empty
/// primary key; otherwise it is read-only (rationale: a row can't be safely
/// addressed in a `WHERE` clause without a PK — see spec §5.3).
#[derive(Debug, Clone)]
pub struct EditContext {
    pub driver: Driver,
    pub schema: String,
    pub table: String,
    pub pk_columns: Vec<String>,
}

/// In-progress inline edit of a single cell.
#[derive(Debug)]
struct CellEdit {
    row: usize,
    col: usize,
    buffer: String,
}

/// A grid that displays `QueryResult` rows and supports inline CRUD editing.
#[derive(Debug, Default)]
pub struct ResultGrid {
    result: Option<QueryResult>,
    cursor_row: usize,
    cursor_col: usize,
    edit_ctx: Option<EditContext>,
    /// Edits to existing rows: `(row, col) -> new display value`.
    edits: HashMap<(usize, usize), String>,
    /// Existing rows marked for deletion.
    deleted: BTreeSet<usize>,
    /// Appended rows, each a per-column `Option<value>` (`None` = unset).
    new_rows: Vec<Vec<Option<String>>>,
    /// Active inline cell edit, if any.
    editing: Option<CellEdit>,
    /// Colors used when rendering.
    palette: Palette,
}

impl ResultGrid {
    /// Create an empty grid.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the color palette used when rendering.
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Replace the displayed result and reset cursor + pending edits.
    ///
    /// The `EditContext` is intentionally **not** cleared here: a refresh of the
    /// same browsed table reuses it. Use [`set_edit_context`] to change it.
    pub fn set_result(&mut self, result: Option<QueryResult>) {
        self.result = result;
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.clear_pending();
    }

    /// Set (or clear) the table context that makes the grid editable. Also
    /// drops any pending edits, since they belong to the previous context.
    pub fn set_edit_context(&mut self, ctx: Option<EditContext>) {
        self.edit_ctx = ctx;
        self.clear_pending();
    }

    fn clear_pending(&mut self) {
        self.edits.clear();
        self.deleted.clear();
        self.new_rows.clear();
        self.editing = None;
    }

    /// Return a reference to the current result, if any.
    pub fn result(&self) -> &Option<QueryResult> {
        &self.result
    }

    /// The table context backing this grid, if it was opened by browsing a
    /// table (used to name the target table for SQL export).
    pub fn edit_context(&self) -> Option<&EditContext> {
        self.edit_ctx.as_ref()
    }

    /// True when the grid can be edited (has context and a primary key).
    pub fn is_editable(&self) -> bool {
        self.edit_ctx
            .as_ref()
            .map(|c| !c.pk_columns.is_empty())
            .unwrap_or(false)
    }

    /// True while a cell is being edited inline.
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// True when there are uncommitted edits, inserts or deletes.
    pub fn has_pending(&self) -> bool {
        !self.edits.is_empty() || !self.deleted.is_empty() || !self.new_rows.is_empty()
    }

    /// Number of rows affected by pending changes (for the status line).
    pub fn pending_count(&self) -> usize {
        let edited_rows: BTreeSet<usize> = self
            .edits
            .keys()
            .map(|(r, _)| *r)
            .filter(|r| !self.deleted.contains(r))
            .collect();
        edited_rows.len() + self.deleted.len() + self.new_rows.len()
    }

    fn existing_rows(&self) -> usize {
        self.result.as_ref().map(|r| r.rows.len()).unwrap_or(0)
    }

    fn total_rows(&self) -> usize {
        self.existing_rows() + self.new_rows.len()
    }

    /// Move the cursor one row up, clamped at the first row.
    pub fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
    }

    /// Move the cursor one row down, clamped at the last row.
    pub fn move_down(&mut self) {
        let max = self.total_rows().saturating_sub(1);
        if self.cursor_row < max {
            self.cursor_row += 1;
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
        self.cursor_row = self.total_rows().saturating_sub(1);
    }

    /// Display value at `(row, col)`, honoring pending edits and new rows.
    fn cell_display(&self, row: usize, col: usize) -> String {
        let Some(result) = &self.result else {
            return String::new();
        };
        let existing = result.rows.len();
        if row < existing {
            if let Some(v) = self.edits.get(&(row, col)) {
                return v.clone();
            }
            return result
                .rows
                .get(row)
                .and_then(|r| r.get(col))
                .map(cell_value_to_string)
                .unwrap_or_default();
        }
        let ni = row - existing;
        match self.new_rows.get(ni).and_then(|r| r.get(col)) {
            Some(Some(v)) => v.clone(),
            _ => String::new(),
        }
    }

    /// Begin editing the cell under the cursor (no-op if not editable).
    pub fn begin_edit(&mut self) {
        if !self.is_editable() {
            return;
        }
        let Some(result) = &self.result else { return };
        if self.cursor_col >= result.columns.len() || self.cursor_row >= self.total_rows() {
            return;
        }
        let buffer = self.cell_display(self.cursor_row, self.cursor_col);
        self.editing = Some(CellEdit {
            row: self.cursor_row,
            col: self.cursor_col,
            buffer,
        });
    }

    /// Feed a key to the active inline edit. Returns `true` while still editing.
    pub fn handle_edit_key(&mut self, key: KeyEvent) -> bool {
        let Some(edit) = self.editing.as_mut() else {
            return false;
        };
        match key.code {
            KeyCode::Esc => {
                self.editing = None;
                false
            }
            KeyCode::Enter => {
                self.commit_edit();
                false
            }
            KeyCode::Backspace => {
                edit.buffer.pop();
                true
            }
            KeyCode::Char(c) => {
                edit.buffer.push(c);
                true
            }
            _ => true,
        }
    }

    fn commit_edit(&mut self) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        let existing = self.existing_rows();
        if edit.row < existing {
            self.edits.insert((edit.row, edit.col), edit.buffer);
        } else if let Some(slot) = self
            .new_rows
            .get_mut(edit.row - existing)
            .and_then(|r| r.get_mut(edit.col))
        {
            *slot = Some(edit.buffer);
        }
    }

    /// Append an empty pending row and move the cursor to it.
    pub fn add_row(&mut self) {
        if !self.is_editable() {
            return;
        }
        let Some(result) = &self.result else { return };
        let ncols = result.columns.len();
        self.new_rows.push(vec![None; ncols]);
        self.cursor_row = self.total_rows() - 1;
        self.cursor_col = 0;
    }

    /// Toggle deletion of the current existing row, or drop a pending new row.
    pub fn mark_delete(&mut self) {
        if !self.is_editable() {
            return;
        }
        let existing = self.existing_rows();
        if self.cursor_row < existing {
            if !self.deleted.insert(self.cursor_row) {
                self.deleted.remove(&self.cursor_row);
            }
        } else {
            let ni = self.cursor_row - existing;
            if ni < self.new_rows.len() {
                self.new_rows.remove(ni);
                let max = self.total_rows().saturating_sub(1);
                self.cursor_row = self.cursor_row.min(max);
            }
        }
    }

    /// Discard all pending edits, inserts and deletes.
    pub fn discard_changes(&mut self) {
        self.clear_pending();
        let max = self.total_rows().saturating_sub(1);
        self.cursor_row = self.cursor_row.min(max);
    }

    /// Build the `UPDATE`/`DELETE`/`INSERT` statements for pending changes.
    ///
    /// Existing rows are keyed by their original primary-key values; new rows
    /// contribute only the columns that were actually set.
    pub fn build_commit_statements(&self) -> Vec<String> {
        let (Some(ctx), Some(result)) = (&self.edit_ctx, &self.result) else {
            return vec![];
        };
        if ctx.pk_columns.is_empty() {
            return vec![];
        }
        let cols: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
        let pk_indices: Vec<(String, usize)> = ctx
            .pk_columns
            .iter()
            .filter_map(|pk| cols.iter().position(|c| *c == pk).map(|i| (pk.clone(), i)))
            .collect();
        if pk_indices.len() != ctx.pk_columns.len() {
            return vec![]; // a PK column is missing from the result; refuse
        }

        let pk_values = |row: usize| -> Vec<(String, String)> {
            pk_indices
                .iter()
                .map(|(name, idx)| (name.clone(), cell_value_to_string(&result.rows[row][*idx])))
                .collect()
        };

        let mut stmts = Vec::new();

        // UPDATEs: group edited cells by row, skipping rows also being deleted.
        let mut edited_rows: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for &(row, col) in self.edits.keys() {
            if row < result.rows.len() && !self.deleted.contains(&row) {
                edited_rows.entry(row).or_default().push(col);
            }
        }
        for (row, mut ecols) in edited_rows {
            ecols.sort_unstable();
            let set_owned: Vec<(String, String)> = ecols
                .iter()
                .map(|&c| (cols[c].to_string(), self.edits[&(row, c)].clone()))
                .collect();
            let pk_owned = pk_values(row);
            stmts.push(sextant_db::build_update(
                ctx.driver,
                &ctx.schema,
                &ctx.table,
                &as_refs(&set_owned),
                &as_refs(&pk_owned),
            ));
        }

        // DELETEs.
        for &row in &self.deleted {
            if row < result.rows.len() {
                let pk_owned = pk_values(row);
                stmts.push(sextant_db::build_delete(
                    ctx.driver,
                    &ctx.schema,
                    &ctx.table,
                    &as_refs(&pk_owned),
                ));
            }
        }

        // INSERTs.
        for nr in &self.new_rows {
            let set_owned: Vec<(String, String)> = nr
                .iter()
                .enumerate()
                .filter_map(|(c, v)| v.as_ref().map(|val| (cols[c].to_string(), val.clone())))
                .collect();
            if set_owned.is_empty() {
                continue;
            }
            stmts.push(sextant_db::build_insert(
                ctx.driver,
                &ctx.schema,
                &ctx.table,
                &as_refs(&set_owned),
            ));
        }

        stmts
    }

    /// Render the grid (or a placeholder) into `area`.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let p = self.palette;
        let Some(ref result) = self.result else {
            let placeholder = Block::default()
                .borders(Borders::NONE)
                .style(Style::default().bg(p.background));
            frame.render_widget(placeholder, area);
            return;
        };

        if result.columns.is_empty() || (result.rows.is_empty() && self.new_rows.is_empty()) {
            let text = Line::from(Span::styled("No results", Style::default().fg(p.muted)));
            let para = ratatui::widgets::Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::NONE)
                    .style(Style::default().bg(p.background)),
            );
            frame.render_widget(para, area);
            return;
        }

        let widths = compute_column_widths(result);
        let existing = result.rows.len();

        let header_cells: Vec<Cell> = result
            .columns
            .iter()
            .enumerate()
            .map(|(i, col)| {
                let style = if i == self.cursor_col {
                    Style::default().fg(p.selection_fg).bg(p.selection_bg)
                } else {
                    Style::default().fg(p.accent)
                };
                Cell::new(col.name.clone()).style(style)
            })
            .collect();
        let header = Row::new(header_cells).style(Style::default().bg(p.background));

        let rows: Vec<Row> = (0..self.total_rows())
            .map(|row_idx| {
                let is_new = row_idx >= existing;
                let is_deleted = self.deleted.contains(&row_idx);
                let cells: Vec<Cell> = (0..result.columns.len())
                    .map(|col_idx| {
                        let editing_here = self
                            .editing
                            .as_ref()
                            .map(|e| e.row == row_idx && e.col == col_idx)
                            .unwrap_or(false);
                        let text = if editing_here {
                            self.editing.as_ref().unwrap().buffer.clone()
                        } else {
                            self.cell_display(row_idx, col_idx)
                        };
                        let is_active = row_idx == self.cursor_row && col_idx == self.cursor_col;
                        let is_edited = self.edits.contains_key(&(row_idx, col_idx));
                        let style = if is_active {
                            Style::default().fg(p.background).bg(p.accent_alt)
                        } else if is_deleted {
                            Style::default()
                                .fg(p.error)
                                .add_modifier(Modifier::CROSSED_OUT)
                        } else if is_new {
                            Style::default().fg(p.success)
                        } else if is_edited {
                            Style::default().fg(p.accent_alt)
                        } else if row_idx == self.cursor_row {
                            Style::default().bg(p.muted)
                        } else {
                            Style::default().fg(p.foreground)
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
                .style(Style::default().bg(p.background)),
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

/// Borrow a `Vec<(String, String)>` as `Vec<(&str, &str)>` for the SQL builders.
fn as_refs(pairs: &[(String, String)]) -> Vec<(&str, &str)> {
    pairs
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect()
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

    use ratatui::crossterm::event::{KeyCode, KeyEvent};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn editable_grid() -> ResultGrid {
        let mut g = ResultGrid::new();
        g.set_result(Some(sample_result()));
        g.set_edit_context(Some(EditContext {
            driver: Driver::Sqlite,
            schema: "main".into(),
            table: "users".into(),
            pk_columns: vec!["id".into()],
        }));
        g
    }

    fn type_into_cell(g: &mut ResultGrid, text: &str) {
        g.begin_edit();
        // Clear whatever is there, then type the new text.
        while g.is_editing() && !g.editing.as_ref().unwrap().buffer.is_empty() {
            g.handle_edit_key(key(KeyCode::Backspace));
        }
        for c in text.chars() {
            g.handle_edit_key(key(KeyCode::Char(c)));
        }
        g.handle_edit_key(key(KeyCode::Enter));
    }

    #[test]
    fn read_only_without_context_or_pk() {
        let mut g = ResultGrid::new();
        g.set_result(Some(sample_result()));
        assert!(!g.is_editable());

        g.set_edit_context(Some(EditContext {
            driver: Driver::Sqlite,
            schema: "main".into(),
            table: "t".into(),
            pk_columns: vec![],
        }));
        assert!(!g.is_editable());
    }

    #[test]
    fn editing_a_cell_generates_update() {
        let mut g = editable_grid();
        g.cursor_row = 0;
        g.cursor_col = 1;
        type_into_cell(&mut g, "Alicia");
        assert!(g.has_pending());

        let stmts = g.build_commit_statements();
        assert_eq!(
            stmts,
            vec![
                "UPDATE \"main\".\"users\" SET \"name\" = 'Alicia' WHERE \"id\" = '1'".to_string()
            ]
        );
    }

    #[test]
    fn esc_cancels_edit_without_pending() {
        let mut g = editable_grid();
        g.cursor_row = 0;
        g.cursor_col = 1;
        g.begin_edit();
        g.handle_edit_key(key(KeyCode::Char('X')));
        let still = g.handle_edit_key(key(KeyCode::Esc));
        assert!(!still);
        assert!(!g.has_pending());
    }

    #[test]
    fn adding_a_row_generates_insert() {
        let mut g = editable_grid();
        g.add_row();
        assert_eq!(g.cursor_row, 2); // 2 existing rows -> new row at index 2

        g.cursor_col = 0;
        type_into_cell(&mut g, "3");
        g.cursor_col = 1;
        type_into_cell(&mut g, "Cara");

        let stmts = g.build_commit_statements();
        assert_eq!(
            stmts,
            vec![
                "INSERT INTO \"main\".\"users\" (\"id\", \"name\") VALUES ('3', 'Cara')"
                    .to_string()
            ]
        );
    }

    #[test]
    fn marking_a_row_generates_delete() {
        let mut g = editable_grid();
        g.cursor_row = 1; // Bob, id = 2
        g.mark_delete();

        let stmts = g.build_commit_statements();
        assert_eq!(
            stmts,
            vec!["DELETE FROM \"main\".\"users\" WHERE \"id\" = '2'".to_string()]
        );
    }

    #[test]
    fn dd_toggles_delete_off() {
        let mut g = editable_grid();
        g.cursor_row = 0;
        g.mark_delete();
        assert!(g.has_pending());
        g.mark_delete();
        assert!(!g.has_pending());
    }

    #[test]
    fn discard_clears_all_pending() {
        let mut g = editable_grid();
        g.cursor_row = 0;
        g.cursor_col = 1;
        type_into_cell(&mut g, "Zoe");
        g.add_row();
        g.cursor_row = 1;
        g.mark_delete();
        assert!(g.has_pending());

        g.discard_changes();
        assert!(!g.has_pending());
        assert!(g.build_commit_statements().is_empty());
    }

    #[test]
    fn editing_pk_uses_original_value_in_where() {
        let mut g = editable_grid();
        g.cursor_row = 0;
        g.cursor_col = 0; // edit the id (PK) itself
        type_into_cell(&mut g, "100");

        let stmts = g.build_commit_statements();
        // SET new id, WHERE on the original id.
        assert_eq!(
            stmts,
            vec!["UPDATE \"main\".\"users\" SET \"id\" = '100' WHERE \"id\" = '1'".to_string()]
        );
    }
}
