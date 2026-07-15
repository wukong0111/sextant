//! Result grid with inline editing.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent},
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders},
};
use sextant_core::{CellValue, Driver, QueryResult};

use crate::palette::Palette;

/// Format for copying the selected grid range to the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyFormat {
    Csv,
    Tsv,
    Json,
    SqlInsert,
}

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
    /// Cursor position in Unicode scalar values (`0..=buffer.chars().count()`).
    cursor: usize,
}

impl CellEdit {
    fn move_cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_cursor_right(&mut self) {
        let len = self.buffer.chars().count();
        if self.cursor < len {
            self.cursor += 1;
        }
    }

    fn move_cursor_home(&mut self) {
        self.cursor = 0;
    }

    fn move_cursor_end(&mut self) {
        self.cursor = self.buffer.chars().count();
    }

    fn insert_char(&mut self, c: char) {
        let len = self.buffer.chars().count();
        if self.cursor >= len {
            self.buffer.push(c);
        } else {
            let idx = self
                .buffer
                .char_indices()
                .nth(self.cursor)
                .map(|(i, _)| i)
                .unwrap_or(self.buffer.len());
            self.buffer.insert(idx, c);
        }
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self
            .buffer
            .char_indices()
            .nth(self.cursor - 1)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let end = self
            .buffer
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.buffer.len());
        self.buffer.drain(start..end);
        self.cursor -= 1;
    }
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
    /// User-overridden column widths: `col_index -> width`.
    column_widths: HashMap<usize, usize>,
    /// Anchor cell of the visual selection (row, col).
    selection_anchor: Option<(usize, usize)>,
    /// Whether a visual selection is currently active.
    selection_active: bool,
    /// Rows selected for full-row operations (toggle with `x`).
    selected_rows: BTreeSet<usize>,
    /// Pivot row for range extension via `X` (`ExtendRowSelection`): the most
    /// recent row selected with `toggle_row_selection`. Cleared together with
    /// `selected_rows`.
    row_selection_anchor: Option<usize>,
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
        self.clear_selection();
        self.clear_row_selection();
        self.column_widths.clear();
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

    fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.selection_active = false;
    }

    /// Enter visual mode, anchoring the selection at the current cursor.
    pub fn enter_visual_mode(&mut self) {
        if self.result.is_some() {
            self.selection_anchor = Some((self.cursor_row, self.cursor_col));
            self.selection_active = true;
        }
    }

    /// Exit visual mode and clear the selection highlight.
    pub fn exit_visual_mode(&mut self) {
        self.clear_selection();
    }

    /// Whether the cell at `(row, col)` lies inside the current selection rectangle.
    pub fn is_cell_selected(&self, row: usize, col: usize) -> bool {
        if !self.selection_active {
            return false;
        }
        let Some((anchor_row, anchor_col)) = self.selection_anchor else {
            return false;
        };
        let min_row = anchor_row.min(self.cursor_row);
        let max_row = anchor_row.max(self.cursor_row);
        let min_col = anchor_col.min(self.cursor_col);
        let max_col = anchor_col.max(self.cursor_col);
        row >= min_row && row <= max_row && col >= min_col && col <= max_col
    }

    /// Return the inclusive selection bounds `(min_row, min_col, max_row, max_col)`.
    pub fn selected_range(&self) -> Option<(usize, usize, usize, usize)> {
        if !self.selection_active {
            return None;
        }
        let (anchor_row, anchor_col) = self.selection_anchor?;
        let min_row = anchor_row.min(self.cursor_row);
        let max_row = anchor_row.max(self.cursor_row);
        let min_col = anchor_col.min(self.cursor_col);
        let max_col = anchor_col.max(self.cursor_col);
        Some((min_row, min_col, max_row, max_col))
    }

    /// Toggle selection of the given full row. Selecting a row updates the
    /// range-extension anchor used by `extend_row_selection_to_cursor`.
    pub fn toggle_row_selection(&mut self, row: usize) {
        if row >= self.total_rows() {
            return;
        }
        if self.selected_rows.insert(row) {
            self.row_selection_anchor = Some(row);
        } else {
            self.selected_rows.remove(&row);
        }
    }

    /// Select every row between the anchor (the last row selected with `x`) and
    /// the cursor, inclusive. The range is added to the existing selection
    /// (union). No-op when there is no anchor or no row is currently selected.
    pub fn extend_row_selection_to_cursor(&mut self) {
        if self.row_selection_anchor.is_none() || self.selected_rows.is_empty() {
            return;
        }
        let anchor = self.row_selection_anchor.unwrap();
        let cursor = self.cursor_row.min(self.total_rows().saturating_sub(1));
        for row in anchor.min(cursor)..=anchor.max(cursor) {
            self.selected_rows.insert(row);
        }
    }

    /// Clear all full-row selections and the range-extension anchor.
    pub fn clear_row_selection(&mut self) {
        self.selected_rows.clear();
        self.row_selection_anchor = None;
    }

    /// Whether the given row is selected as a full row.
    pub fn is_row_selected(&self, row: usize) -> bool {
        self.selected_rows.contains(&row)
    }

    /// Whether any full rows are currently selected.
    pub fn has_row_selection(&self) -> bool {
        !self.selected_rows.is_empty()
    }

    /// Number of full rows currently selected.
    pub fn selected_row_count(&self) -> usize {
        self.selected_rows.len()
    }

    /// Mark all selected full rows as deleted and clear the selection.
    pub fn delete_selected_rows(&mut self) {
        if !self.is_editable() || self.selected_rows.is_empty() {
            return;
        }
        for &row in &self.selected_rows {
            if row < self.existing_rows() {
                self.deleted.insert(row);
            }
        }
        self.clear_row_selection();
    }

    /// Copy the selected range in the requested format.
    pub fn copy(&self, format: CopyFormat) -> Result<String, String> {
        match format {
            CopyFormat::Csv => self.copy_as_csv(),
            CopyFormat::Tsv => self.copy_as_tsv(),
            CopyFormat::Json => self.copy_as_json(),
            CopyFormat::SqlInsert => self.copy_as_sql_insert(),
        }
    }

    /// Copy the selected full rows in the requested format.
    pub fn copy_selected_rows(&self, format: CopyFormat) -> Result<String, String> {
        if self.selected_rows.is_empty() {
            return Err("no rows selected".to_string());
        }
        match format {
            CopyFormat::Csv => self.copy_selected_rows_delimited(b','),
            CopyFormat::Tsv => self.copy_selected_rows_delimited(b'\t'),
            CopyFormat::Json => self.copy_selected_rows_json(),
            CopyFormat::SqlInsert => self.copy_selected_rows_sql_insert(),
        }
    }

    /// Copy the plain text of the cell currently under the cursor.
    pub fn copy_current_cell(&self) -> Result<String, String> {
        let result = self.result.as_ref().ok_or("no result")?;
        if self.cursor_row >= self.total_rows() || self.cursor_col >= result.columns.len() {
            return Err("cursor out of bounds".to_string());
        }
        Ok(self.cell_display(self.cursor_row, self.cursor_col))
    }

    fn copy_selected_rows_delimited(&self, delimiter: u8) -> Result<String, String> {
        let result = self.result.as_ref().ok_or("no result")?;
        let mut wtr = csv::WriterBuilder::new()
            .delimiter(delimiter)
            .from_writer(Vec::new());

        let header: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
        wtr.write_record(&header).map_err(|e| e.to_string())?;

        for &row in &self.selected_rows {
            let mut record = Vec::new();
            for col in 0..result.columns.len() {
                record.push(self.cell_display(row, col));
            }
            wtr.write_record(&record).map_err(|e| e.to_string())?;
        }

        wtr.into_inner()
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .map_err(|e| e.to_string())
    }

    fn copy_selected_rows_json(&self) -> Result<String, String> {
        let result = self.result.as_ref().ok_or("no result")?;
        let mut rows = Vec::new();
        for &row in &self.selected_rows {
            let mut obj = serde_json::Map::new();
            for (col, col_meta) in result.columns.iter().enumerate() {
                obj.insert(col_meta.name.clone(), self.json_value(row, col));
            }
            rows.push(serde_json::Value::Object(obj));
        }
        serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())
    }

    fn copy_selected_rows_sql_insert(&self) -> Result<String, String> {
        let result = self.result.as_ref().ok_or("no result")?;
        let ctx = self
            .edit_ctx
            .as_ref()
            .ok_or("SQL INSERT requires a browsed table")?;

        let cols: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
        let mut stmts = Vec::new();
        for &row in &self.selected_rows {
            let set_owned: Vec<(String, String)> = cols
                .iter()
                .enumerate()
                .map(|(col, name)| (name.to_string(), self.cell_display(row, col)))
                .collect();
            let set_refs: Vec<(&str, &str)> = set_owned
                .iter()
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect();
            stmts.push(sextant_db::build_insert(
                ctx.driver,
                &ctx.schema,
                &ctx.table,
                &set_refs,
            ));
        }
        Ok(stmts.join(";\n"))
    }

    fn copy_as_csv(&self) -> Result<String, String> {
        self.copy_delimited(b',')
    }

    fn copy_as_tsv(&self) -> Result<String, String> {
        self.copy_delimited(b'\t')
    }

    fn copy_delimited(&self, delimiter: u8) -> Result<String, String> {
        let Some((min_r, min_c, max_r, max_c)) = self.selected_range() else {
            return Err("no selection".to_string());
        };
        let result = self.result.as_ref().ok_or("no result")?;
        let mut wtr = csv::WriterBuilder::new()
            .delimiter(delimiter)
            .from_writer(Vec::new());

        // Header row.
        let mut header = Vec::new();
        for c in min_c..=max_c {
            if let Some(col) = result.columns.get(c) {
                header.push(col.name.clone());
            }
        }
        wtr.write_record(&header).map_err(|e| e.to_string())?;

        // Data rows.
        for r in min_r..=max_r {
            let mut row = Vec::new();
            for c in min_c..=max_c {
                row.push(self.cell_display(r, c));
            }
            wtr.write_record(&row).map_err(|e| e.to_string())?;
        }

        wtr.into_inner()
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .map_err(|e| e.to_string())
    }

    fn copy_as_json(&self) -> Result<String, String> {
        let Some((min_r, min_c, max_r, max_c)) = self.selected_range() else {
            return Err("no selection".to_string());
        };
        let result = self.result.as_ref().ok_or("no result")?;

        let mut rows = Vec::new();
        for r in min_r..=max_r {
            let mut obj = serde_json::Map::new();
            for c in min_c..=max_c {
                let col_name = result
                    .columns
                    .get(c)
                    .map(|col| col.name.clone())
                    .unwrap_or_else(|| format!("col_{c}"));
                let value = self.json_value(r, c);
                obj.insert(col_name, value);
            }
            rows.push(serde_json::Value::Object(obj));
        }

        serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())
    }

    fn json_value(&self, row: usize, col: usize) -> serde_json::Value {
        let Some(result) = &self.result else {
            return serde_json::Value::Null;
        };
        let existing = result.rows.len();
        if row < existing {
            if let Some(v) = self.edits.get(&(row, col)) {
                return serde_json::Value::String(v.clone());
            }
            if let Some(cell) = result.rows.get(row).and_then(|r| r.get(col)) {
                return match cell {
                    CellValue::Null => serde_json::Value::Null,
                    CellValue::Bool(b) => serde_json::Value::Bool(*b),
                    CellValue::I64(v) => serde_json::Value::Number((*v).into()),
                    CellValue::F64(v) => serde_json::Value::Number(
                        serde_json::Number::from_f64(*v)
                            .unwrap_or_else(|| serde_json::Number::from(0)),
                    ),
                    CellValue::String(s) => serde_json::Value::String(s.clone()),
                    CellValue::Bytes(_) => serde_json::Value::String("<binary>".to_string()),
                };
            }
        } else {
            let ni = row - existing;
            if let Some(Some(v)) = self.new_rows.get(ni).and_then(|r| r.get(col)) {
                return serde_json::Value::String(v.clone());
            }
        }
        serde_json::Value::Null
    }

    fn copy_as_sql_insert(&self) -> Result<String, String> {
        let Some((min_r, min_c, max_r, max_c)) = self.selected_range() else {
            return Err("no selection".to_string());
        };
        let result = self.result.as_ref().ok_or("no result")?;
        let ctx = self
            .edit_ctx
            .as_ref()
            .ok_or("SQL INSERT requires a browsed table")?;

        let cols: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
        let mut stmts = Vec::new();
        for r in min_r..=max_r {
            let mut set_owned = Vec::new();
            for c in min_c..=max_c {
                if let Some(name) = cols.get(c) {
                    set_owned.push((name.to_string(), self.cell_display(r, c)));
                }
            }
            if !set_owned.is_empty() {
                let set_refs: Vec<(&str, &str)> = set_owned
                    .iter()
                    .map(|(a, b)| (a.as_str(), b.as_str()))
                    .collect();
                stmts.push(sextant_db::build_insert(
                    ctx.driver,
                    &ctx.schema,
                    &ctx.table,
                    &set_refs,
                ));
            }
        }
        Ok(stmts.join(";\n"))
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
        let cursor = buffer.chars().count();
        self.editing = Some(CellEdit {
            row: self.cursor_row,
            col: self.cursor_col,
            buffer,
            cursor,
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
                edit.backspace();
                true
            }
            KeyCode::Char(c) => {
                edit.insert_char(c);
                true
            }
            KeyCode::Left => {
                edit.move_cursor_left();
                true
            }
            KeyCode::Right => {
                edit.move_cursor_right();
                true
            }
            KeyCode::Home => {
                edit.move_cursor_home();
                true
            }
            KeyCode::End => {
                edit.move_cursor_end();
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
        // Every column's original value for `row`, used as the full-row match in
        // a DELETE's optimistic WHERE.
        let original_row = |row: usize| -> Vec<(String, String)> {
            cols.iter()
                .enumerate()
                .map(|(i, name)| (name.to_string(), cell_value_to_string(&result.rows[row][i])))
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
            // Optimistic-concurrency WHERE: primary key plus the *original*
            // values of the edited columns, so a concurrent change to any of
            // them makes the UPDATE affect zero rows.
            let mut where_owned = pk_values(row);
            for &c in &ecols {
                let name = cols[c].to_string();
                if !where_owned.iter().any(|(n, _)| *n == name) {
                    where_owned.push((name, cell_value_to_string(&result.rows[row][c])));
                }
            }
            stmts.push(sextant_db::build_update(
                ctx.driver,
                &ctx.schema,
                &ctx.table,
                &as_refs(&set_owned),
                &as_refs(&where_owned),
            ));
        }

        // DELETEs: match the full original row (optimistic concurrency).
        for &row in &self.deleted {
            if row < result.rows.len() {
                let where_owned = original_row(row);
                stmts.push(sextant_db::build_delete(
                    ctx.driver,
                    &ctx.schema,
                    &ctx.table,
                    &as_refs(&where_owned),
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

    /// Widen the current column by 2 cells.
    pub fn widen_column(&mut self) {
        let Some(ref result) = self.result else {
            return;
        };
        if self.cursor_col >= result.columns.len() {
            return;
        }
        let auto = compute_auto_column_width(result, self.cursor_col);
        let entry = self.column_widths.entry(self.cursor_col).or_insert(auto);
        *entry = (*entry + 2).clamp(3, 200);
    }

    /// Narrow the current column by 2 cells.
    pub fn narrow_column(&mut self) {
        let Some(ref result) = self.result else {
            return;
        };
        if self.cursor_col >= result.columns.len() {
            return;
        }
        let auto = compute_auto_column_width(result, self.cursor_col);
        let entry = self.column_widths.entry(self.cursor_col).or_insert(auto);
        *entry = entry.saturating_sub(2).max(3);
    }

    /// Reset the current column to its auto-fitted width.
    pub fn auto_fit_column(&mut self) {
        self.column_widths.remove(&self.cursor_col);
    }

    /// Reset all columns to their auto-fitted widths.
    pub fn auto_fit_all(&mut self) {
        self.column_widths.clear();
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

        let widths = compute_column_widths(result, &self.column_widths);
        let existing = result.rows.len();

        // Leading non-interactive gutter that numbers the visible rows (1-based).
        let gutter_width = self.total_rows().to_string().len().max(1) as u16;

        // Horizontal scroll: derive the first visible column each frame so the
        // selected cell stays on screen on tables wider than the viewport. The
        // gutter reserves space on the left, so only the remainder is available.
        let avail_width = area.width.saturating_sub(gutter_width + 1);
        let offset = first_visible_column(&widths, self.cursor_col, avail_width, 1);

        // Vertical scroll: the header occupies one line, so the data window is
        // `area.height - 1` rows. Derive the top row each frame (stateless, like
        // the horizontal case) so the selected row never leaves the viewport.
        let visible_rows = (area.height as usize).saturating_sub(1);
        let row_offset = first_visible_row(self.cursor_row, visible_rows);

        // Paint the background first so empty areas are filled.
        frame.render_widget(
            Block::default()
                .borders(Borders::NONE)
                .style(Style::default().bg(p.background)),
            area,
        );

        // Paint the fixed row-number gutter before the (scrollable) data columns.
        let gutter_style = Style::default().fg(p.muted);
        frame.render_widget(
            ratatui::widgets::Paragraph::new("#")
                .alignment(ratatui::layout::Alignment::Right)
                .style(gutter_style),
            Rect::new(area.x, area.y, gutter_width, 1),
        );
        let total = self.total_rows();
        for row_idx in row_offset..total {
            let view_idx = row_idx - row_offset;
            let row_y = area.y + 1 + view_idx as u16;
            if row_y >= area.y + area.height {
                break;
            }
            frame.render_widget(
                ratatui::widgets::Paragraph::new(format!("{}", row_idx + 1))
                    .alignment(ratatui::layout::Alignment::Right)
                    .style(gutter_style),
                Rect::new(area.x, row_y, gutter_width, 1),
            );
        }

        let mut x = area.x + gutter_width + 1;
        for (col_idx, col) in result.columns.iter().enumerate().skip(offset) {
            let col_width = widths[col_idx] as u16;

            // Column spacing (1 cell) — skip if no room left.
            if col_idx > offset {
                if x >= area.x + area.width {
                    break;
                }
                x += 1;
            }

            let remaining = area.x + area.width - x;
            if remaining == 0 {
                break;
            }
            let render_width = col_width.min(remaining);

            // Header cell.
            let header_style = if col_idx == self.cursor_col {
                Style::default().fg(p.selection_fg).bg(p.selection_bg)
            } else {
                Style::default().fg(p.accent)
            };
            frame.render_widget(
                ratatui::widgets::Paragraph::new(col.name.clone()).style(header_style),
                Rect::new(x, area.y, render_width, 1),
            );

            // Data cells.
            for row_idx in row_offset..self.total_rows() {
                let view_idx = row_idx - row_offset;
                let row_y = area.y + 1 + view_idx as u16;
                if row_y >= area.y + area.height {
                    break;
                }

                let editing_here = self
                    .editing
                    .as_ref()
                    .map(|e| e.row == row_idx && e.col == col_idx)
                    .unwrap_or(false);
                let is_active = row_idx == self.cursor_row && col_idx == self.cursor_col;
                let is_edited = self.edits.contains_key(&(row_idx, col_idx));
                let is_deleted = self.deleted.contains(&row_idx);
                let is_new = row_idx >= existing;
                let is_row_selected = self.is_row_selected(row_idx);
                let base_style = if is_active {
                    Style::default().fg(p.background).bg(p.accent_alt)
                } else if is_deleted {
                    Style::default()
                        .fg(p.error)
                        .add_modifier(Modifier::CROSSED_OUT)
                } else if is_new {
                    Style::default().fg(p.success)
                } else if is_edited {
                    Style::default().fg(p.accent_alt)
                } else if is_row_selected {
                    Style::default().fg(p.selection_fg).bg(p.selection_bg)
                } else if row_idx == self.cursor_row {
                    Style::default().bg(p.muted)
                } else {
                    Style::default().fg(p.foreground)
                };
                let style =
                    if !is_active && !is_row_selected && self.is_cell_selected(row_idx, col_idx) {
                        base_style.bg(p.selection_bg)
                    } else {
                        base_style
                    };
                if editing_here {
                    let edit = self.editing.as_ref().unwrap();
                    let cursor_style = Style::default().fg(p.background).bg(p.foreground);
                    frame.render_widget(
                        editing_paragraph(edit, style, cursor_style, render_width),
                        Rect::new(x, row_y, render_width, 1),
                    );
                } else {
                    let text = self.cell_display(row_idx, col_idx);
                    frame.render_widget(
                        ratatui::widgets::Paragraph::new(text).style(style),
                        Rect::new(x, row_y, render_width, 1),
                    );
                }
            }

            x += col_width;
        }
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

/// Render an in-progress cell edit as a one-line paragraph with a visible cursor.
///
/// The character under the cursor is drawn with `cursor_style`; when the cursor
/// sits at the end of the buffer `cursor_style` is applied to a trailing space
/// so it shows as a block cursor. If the buffer is wider than `width`, the
/// visible window is shifted so the cursor always stays in view.
fn editing_paragraph(
    edit: &CellEdit,
    base_style: Style,
    cursor_style: Style,
    width: u16,
) -> ratatui::widgets::Paragraph<'_> {
    let chars: Vec<char> = edit.buffer.chars().collect();
    let cursor = edit.cursor.min(chars.len());
    let width = width as usize;

    // Total logical width: buffer length plus one cell for the end-of-buffer
    // block cursor.
    let total_width = chars.len() + 1;
    let start = if total_width <= width {
        0
    } else if cursor == chars.len() {
        // Keep the block cursor at the right edge; show the trailing chars.
        chars.len() + 1 - width
    } else {
        // Show as much leading text as possible while keeping the cursor char
        // inside the window.
        cursor.saturating_sub(width - 1)
    };

    let mut spans = Vec::new();
    let end = (start + width).min(chars.len());

    if cursor < chars.len() {
        // Text before the cursor character.
        if start < cursor {
            let before: String = chars[start..cursor].iter().collect();
            spans.push(Span::styled(before, base_style));
        }
        // Cursor character.
        spans.push(Span::styled(chars[cursor].to_string(), cursor_style));
        // Text after the cursor character.
        if cursor + 1 < end {
            let after: String = chars[cursor + 1..end].iter().collect();
            spans.push(Span::styled(after, base_style));
        }
    } else {
        // Cursor at end: show trailing text plus a block cursor.
        if start < chars.len() {
            let visible: String = chars[start..chars.len()].iter().collect();
            spans.push(Span::styled(visible, base_style));
        }
        spans.push(Span::styled(" ", cursor_style));
    }

    ratatui::widgets::Paragraph::new(Line::from(spans))
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

/// Compute the auto-fitted width for a single column.
fn compute_auto_column_width(result: &QueryResult, col: usize) -> usize {
    let mut w = result.columns.get(col).map(|c| c.name.len()).unwrap_or(3);
    for row in &result.rows {
        if let Some(cell) = row.get(col) {
            w = w.max(cell_value_to_string(cell).len());
        }
    }
    w.clamp(3, 40)
}

/// Compute per-column display widths, blending auto-fit with user overrides.
fn compute_column_widths(result: &QueryResult, overrides: &HashMap<usize, usize>) -> Vec<usize> {
    (0..result.columns.len())
        .map(|i| {
            let auto = compute_auto_column_width(result, i);
            overrides.get(&i).copied().unwrap_or(auto).clamp(3, 200)
        })
        .collect()
}

/// First column to render so that `cursor_col` stays within `area_width`.
///
/// Returns the smallest left-anchored `offset` such that columns
/// `offset..=cursor_col` fit in `area_width` (accounting for `spacing` between
/// columns, ratatui's default `column_spacing`). Keeps the maximum number of
/// leading columns while guaranteeing the cursor is visible.
fn first_visible_column(
    widths: &[usize],
    cursor_col: usize,
    area_width: u16,
    spacing: u16,
) -> usize {
    let cursor_col = cursor_col.min(widths.len().saturating_sub(1));
    let area = area_width as usize;
    let spacing = spacing as usize;
    let mut offset = 0;
    while offset < cursor_col {
        // Total width of columns offset..=cursor_col, with spacing between them.
        let span: usize =
            widths[offset..=cursor_col].iter().sum::<usize>() + spacing * (cursor_col - offset);
        if span <= area {
            break;
        }
        offset += 1;
    }
    offset
}

/// Smallest top-anchored row offset such that rows `offset..=cursor_row` fit in
/// `visible_rows` data lines. Mirrors `first_visible_column` but for uniform
/// 1-line-tall rows: keeps the maximum number of leading rows visible while
/// guaranteeing the cursor row stays on screen.
fn first_visible_row(cursor_row: usize, visible_rows: usize) -> usize {
    cursor_row.saturating_sub(visible_rows.saturating_sub(1))
}

/// Number of columns that fit in `area_width` starting from `offset`.
///
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend, style::Color};
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
    fn grid_renders_row_number_column() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));

        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();

        let buf = terminal.backend().buffer();
        let w = buf.area.width as usize;
        // Header row y=0; data rows start at y=1.
        let header: String = (0..w).map(|x| buf[(x as u16, 0)].symbol()).collect();
        assert!(
            header.starts_with('#'),
            "header should start with '#': {header:?}"
        );
        let row0: String = (0..w).map(|x| buf[(x as u16, 1)].symbol()).collect();
        assert!(
            row0.starts_with('1'),
            "first data row should start with '1': {row0:?}"
        );
        let row1: String = (0..w).map(|x| buf[(x as u16, 2)].symbol()).collect();
        assert!(
            row1.starts_with('2'),
            "second data row should start with '2': {row1:?}"
        );
    }

    #[test]
    fn row_number_column_counts_new_rows() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut grid = editable_grid();
        grid.add_row(); // total_rows() is now 3

        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();

        let buf = terminal.backend().buffer();
        let w = buf.area.width as usize;
        // The appended row is at index 2, rendered at y=3, and must be numbered "3".
        let new_row: String = (0..w).map(|x| buf[(x as u16, 3)].symbol()).collect();
        assert!(
            new_row.starts_with('3'),
            "appended row should be numbered '3': {new_row:?}"
        );
    }

    #[test]
    fn cursor_cannot_enter_row_number_column() {
        let mut grid = editable_grid();
        grid.cursor_col = 0;
        grid.move_left();
        assert_eq!(
            grid.cursor_col, 0,
            "cursor_col must not go below 0 into the gutter"
        );
    }

    #[test]
    fn first_visible_column_no_scroll_when_all_fit() {
        // Three narrow columns easily fit in a wide area.
        let widths = vec![3, 3, 3];
        assert_eq!(first_visible_column(&widths, 0, 80, 1), 0);
        assert_eq!(first_visible_column(&widths, 2, 80, 1), 0);
    }

    #[test]
    fn first_visible_column_scrolls_to_show_cursor() {
        // Five 10-wide columns; a 25-wide area shows ~2 columns at a time.
        let widths = vec![10, 10, 10, 10, 10];
        // Cursor on the last column must scroll so that column fits.
        let offset = first_visible_column(&widths, 4, 25, 1);
        assert!(
            offset > 0,
            "expected horizontal scroll, got offset {offset}"
        );
        // Columns offset..=4 must fit in 25.
        let span: usize = widths[offset..=4].iter().sum::<usize>() + (4 - offset);
        assert!(span <= 25, "visible span {span} exceeds area");
        // And it must be the *minimal* offset: one less would overflow.
        let prev = offset - 1;
        let span_prev: usize = widths[prev..=4].iter().sum::<usize>() + (4 - prev);
        assert!(span_prev > 25, "offset not minimal");
    }

    #[test]
    fn grid_scrolls_to_keep_cursor_visible() {
        let result = QueryResult {
            columns: vec![
                Column {
                    name: "alpha".into(),
                    type_name: "text".into(),
                },
                Column {
                    name: "bravo".into(),
                    type_name: "text".into(),
                },
                Column {
                    name: "charlie".into(),
                    type_name: "text".into(),
                },
                Column {
                    name: "omega".into(),
                    type_name: "text".into(),
                },
            ],
            rows: vec![vec![
                CellValue::String("a".into()),
                CellValue::String("b".into()),
                CellValue::String("c".into()),
                CellValue::String("z".into()),
            ]],
            rows_affected: None,
        };

        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut grid = ResultGrid::new();
        grid.set_result(Some(result));
        grid.cursor_col = 3; // last column, off-screen at width 20

        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("omega"),
            "selected last column should be visible: {text}"
        );
        assert!(
            !text.contains("alpha"),
            "first column should have scrolled out of view: {text}"
        );
    }

    #[test]
    fn first_visible_row_no_scroll_when_all_fit() {
        // Viewport tall enough to show every row never scrolls.
        assert_eq!(first_visible_row(0, 10), 0);
        assert_eq!(first_visible_row(4, 10), 0);
    }

    #[test]
    fn first_visible_row_scrolls_to_show_cursor() {
        // 5 visible rows; cursor at row 7 must scroll so it sits at the bottom.
        let offset = first_visible_row(7, 5);
        assert_eq!(
            offset, 3,
            "cursor 7 with 5 visible rows → offset 3 (rows 3..=7)"
        );
        // One row above must not scroll past the cursor.
        assert_eq!(
            first_visible_row(4, 5),
            0,
            "cursor 4 still fits from the top"
        );
    }

    #[test]
    fn grid_scrolls_vertically_to_keep_cursor_visible() {
        // A tall result: 6 rows, viewport 3 lines (1 header + 2 data rows).
        let result = QueryResult {
            columns: vec![Column {
                name: "v".into(),
                type_name: "text".into(),
            }],
            rows: (0..6)
                .map(|i| vec![CellValue::String(format!("row{i}"))])
                .collect(),
            rows_affected: None,
        };

        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut grid = ResultGrid::new();
        grid.set_result(Some(result));
        grid.cursor_row = 4; // beyond the 2 visible data rows

        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();

        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("row4"),
            "selected row 4 should be visible: {text}"
        );
        assert!(
            !text.contains("row0"),
            "top row should have scrolled out of view: {text}"
        );
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
        let widths = compute_column_widths(&result, &HashMap::new());
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
        // Optimistic WHERE: PK plus the edited column's original value.
        assert_eq!(
            stmts,
            vec![
                "UPDATE \"main\".\"users\" SET \"name\" = 'Alicia' WHERE \"id\" = '1' AND \"name\" = 'Alice'".to_string()
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
        // Optimistic WHERE: the full original row (id + name).
        assert_eq!(
            stmts,
            vec![
                "DELETE FROM \"main\".\"users\" WHERE \"id\" = '2' AND \"name\" = 'Bob'"
                    .to_string()
            ]
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

    #[test]
    fn widen_column_changes_rendered_width() {
        let result = QueryResult {
            columns: vec![
                Column {
                    name: "id".into(),
                    type_name: "int".into(),
                },
                Column {
                    name: "name".into(),
                    type_name: "text".into(),
                },
                Column {
                    name: "email".into(),
                    type_name: "text".into(),
                },
            ],
            rows: vec![
                vec![
                    CellValue::I64(1),
                    CellValue::String("Alice".into()),
                    CellValue::String("alice@example.com".into()),
                ],
                vec![
                    CellValue::I64(2),
                    CellValue::String("Bob".into()),
                    CellValue::String("bob@example.com".into()),
                ],
            ],
            rows_affected: None,
        };

        let backend = TestBackend::new(40, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut grid = ResultGrid::new();
        grid.set_result(Some(result));
        grid.cursor_col = 1; // name

        // Render before widen.
        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();
        let buf_before = terminal.backend().buffer().clone();

        grid.widen_column();

        // Render after widen.
        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();
        let buf_after = terminal.backend().buffer();

        // The buffers should differ because the column width changed.
        assert_ne!(
            buf_before.content, buf_after.content,
            "widening column 1 should change the rendered buffer"
        );
    }

    #[test]
    fn narrow_column_changes_rendered_width() {
        let result = QueryResult {
            columns: vec![
                Column {
                    name: "id".into(),
                    type_name: "int".into(),
                },
                Column {
                    name: "name".into(),
                    type_name: "text".into(),
                },
                Column {
                    name: "email".into(),
                    type_name: "text".into(),
                },
            ],
            rows: vec![vec![
                CellValue::I64(1),
                CellValue::String("Alice".into()),
                CellValue::String("alice@example.com".into()),
            ]],
            rows_affected: None,
        };

        let backend = TestBackend::new(40, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut grid = ResultGrid::new();
        grid.set_result(Some(result));
        grid.cursor_col = 2; // email

        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();
        let buf_before = terminal.backend().buffer().clone();

        grid.narrow_column();

        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();
        let buf_after = terminal.backend().buffer();

        assert_ne!(
            buf_before.content, buf_after.content,
            "narrowing column 2 should change the rendered buffer"
        );
    }

    #[test]
    fn visual_mode_selects_single_cell() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.enter_visual_mode();
        assert!(grid.is_cell_selected(0, 0));
        assert!(!grid.is_cell_selected(0, 1));
        assert!(!grid.is_cell_selected(1, 0));
    }

    #[test]
    fn visual_mode_expands_rectangle() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.enter_visual_mode(); // anchor at (0, 0)
        grid.move_down(); // cursor at (1, 0)
        grid.move_right(); // cursor at (1, 1)
        assert!(grid.is_cell_selected(0, 0));
        assert!(grid.is_cell_selected(0, 1));
        assert!(grid.is_cell_selected(1, 0));
        assert!(grid.is_cell_selected(1, 1));
        assert!(!grid.is_cell_selected(2, 0));
    }

    #[test]
    fn visual_mode_reverse_selection() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.cursor_row = 1;
        grid.cursor_col = 1;
        grid.enter_visual_mode(); // anchor at (1, 1)
        grid.move_up(); // cursor at (0, 1)
        grid.move_left(); // cursor at (0, 0)
        assert!(grid.is_cell_selected(0, 0));
        assert!(grid.is_cell_selected(0, 1));
        assert!(grid.is_cell_selected(1, 0));
        assert!(grid.is_cell_selected(1, 1));
    }

    #[test]
    fn copy_as_csv_basic() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.enter_visual_mode();
        grid.move_down();
        grid.move_right();
        let csv = grid.copy(CopyFormat::Csv).unwrap();
        assert!(csv.contains("id,name"));
        assert!(csv.contains("1,Alice"));
        assert!(csv.contains("2,Bob"));
    }

    #[test]
    fn copy_as_tsv_basic() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.enter_visual_mode();
        grid.move_down();
        grid.move_right();
        let tsv = grid.copy(CopyFormat::Tsv).unwrap();
        assert_eq!(tsv, "id\tname\n1\tAlice\n2\tBob\n");
    }

    #[test]
    fn copy_as_json_basic() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.enter_visual_mode();
        grid.move_down();
        grid.move_right();
        let json = grid.copy(CopyFormat::Json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[0]["name"], "Alice");
        assert_eq!(arr[1]["id"], 2);
        assert_eq!(arr[1]["name"], "Bob");
    }

    #[test]
    fn copy_as_sql_insert_basic() {
        let mut grid = editable_grid();
        grid.enter_visual_mode();
        grid.move_down();
        grid.move_right();
        let sql = grid.copy(CopyFormat::SqlInsert).unwrap();
        assert_eq!(
            sql,
            "INSERT INTO \"main\".\"users\" (\"id\", \"name\") VALUES ('1', 'Alice');\n\
             INSERT INTO \"main\".\"users\" (\"id\", \"name\") VALUES ('2', 'Bob')"
        );
    }

    #[test]
    fn copy_without_selection_fails() {
        let grid = ResultGrid::new();
        assert!(grid.copy(CopyFormat::Csv).is_err());
    }

    #[test]
    fn toggle_row_selection_adds_and_removes_row() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));

        grid.toggle_row_selection(0);
        assert!(grid.is_row_selected(0));
        assert!(!grid.is_row_selected(1));
        assert_eq!(grid.selected_row_count(), 1);

        grid.toggle_row_selection(1);
        assert!(grid.is_row_selected(0));
        assert!(grid.is_row_selected(1));
        assert_eq!(grid.selected_row_count(), 2);

        grid.toggle_row_selection(0);
        assert!(!grid.is_row_selected(0));
        assert!(grid.is_row_selected(1));
        assert_eq!(grid.selected_row_count(), 1);
    }

    #[test]
    fn toggle_row_selection_out_of_bounds_is_noop() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.toggle_row_selection(99);
        assert!(!grid.has_row_selection());
    }

    #[test]
    fn clear_row_selection_clears_all() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.toggle_row_selection(0);
        grid.toggle_row_selection(1);
        grid.clear_row_selection();
        assert!(!grid.has_row_selection());
    }

    #[test]
    fn set_result_clears_row_selection() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.toggle_row_selection(0);
        grid.set_result(Some(sample_result()));
        assert!(!grid.has_row_selection());
    }

    #[test]
    fn copy_selected_rows_as_csv() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.toggle_row_selection(1);
        let csv = grid.copy_selected_rows(CopyFormat::Csv).unwrap();
        assert!(csv.contains("id,name"));
        assert!(!csv.contains("1,Alice"));
        assert!(csv.contains("2,Bob"));
    }

    #[test]
    fn copy_selected_rows_as_tsv() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.toggle_row_selection(0);
        let tsv = grid.copy_selected_rows(CopyFormat::Tsv).unwrap();
        assert_eq!(tsv, "id\tname\n1\tAlice\n");
    }

    #[test]
    fn copy_selected_rows_as_json() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.toggle_row_selection(1);
        let json = grid.copy_selected_rows(CopyFormat::Json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], 2);
        assert_eq!(arr[0]["name"], "Bob");
    }

    #[test]
    fn copy_selected_rows_as_sql_insert() {
        let mut grid = editable_grid();
        grid.toggle_row_selection(0);
        grid.toggle_row_selection(1);
        let sql = grid.copy_selected_rows(CopyFormat::SqlInsert).unwrap();
        assert_eq!(
            sql,
            "INSERT INTO \"main\".\"users\" (\"id\", \"name\") VALUES ('1', 'Alice');\n\
             INSERT INTO \"main\".\"users\" (\"id\", \"name\") VALUES ('2', 'Bob')"
        );
    }

    #[test]
    fn copy_selected_rows_without_selection_fails() {
        let grid = ResultGrid::new();
        assert!(grid.copy_selected_rows(CopyFormat::Csv).is_err());
    }

    #[test]
    fn delete_selected_rows_marks_deleted() {
        let mut grid = editable_grid();
        grid.toggle_row_selection(0);
        grid.toggle_row_selection(1);
        grid.delete_selected_rows();
        assert!(grid.deleted.contains(&0));
        assert!(grid.deleted.contains(&1));
        assert!(!grid.has_row_selection());
    }

    #[test]
    fn delete_selected_rows_requires_editability() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.toggle_row_selection(0);
        grid.delete_selected_rows();
        assert!(!grid.deleted.contains(&0));
    }

    #[test]
    fn row_selection_independent_from_visual_selection() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.toggle_row_selection(0);
        grid.enter_visual_mode();
        grid.move_down();
        grid.move_right();
        assert!(grid.is_row_selected(0));
        assert!(grid.is_cell_selected(0, 1));
        grid.exit_visual_mode();
        assert!(grid.is_row_selected(0));
        assert!(!grid.is_cell_selected(0, 1));
    }

    /// A 5-row result for range-selection tests.
    fn grid_with_rows(n: usize) -> ResultGrid {
        let mut g = ResultGrid::new();
        g.set_result(Some(QueryResult {
            columns: vec![Column {
                name: "v".into(),
                type_name: "text".into(),
            }],
            rows: (0..n)
                .map(|i| vec![CellValue::String(format!("r{i}"))])
                .collect(),
            rows_affected: None,
        }));
        g
    }

    #[test]
    fn extend_row_selection_fills_range_to_cursor() {
        let mut grid = grid_with_rows(5);
        // Select row 1 -> becomes the anchor; move cursor to row 3 and extend.
        grid.toggle_row_selection(1);
        grid.cursor_row = 3;
        grid.extend_row_selection_to_cursor();

        assert!(grid.is_row_selected(1));
        assert!(grid.is_row_selected(2));
        assert!(grid.is_row_selected(3));
        assert_eq!(grid.selected_row_count(), 3);
    }

    #[test]
    fn extend_row_selection_unions_with_existing() {
        let mut grid = grid_with_rows(5);
        // Row 0 selected independently; then anchor row 1, extend to row 3.
        grid.toggle_row_selection(0);
        grid.toggle_row_selection(1);
        grid.cursor_row = 3;
        grid.extend_row_selection_to_cursor();

        // The range 1..=3 is added; the pre-existing row 0 stays selected.
        assert!(grid.is_row_selected(0));
        assert!(grid.is_row_selected(1));
        assert!(grid.is_row_selected(2));
        assert!(grid.is_row_selected(3));
        assert!(!grid.is_row_selected(4));
        assert_eq!(grid.selected_row_count(), 4);
    }

    #[test]
    fn extend_row_selection_without_anchor_is_noop() {
        let mut grid = grid_with_rows(5);
        // No row ever selected with `x` -> no anchor -> extend is a no-op.
        grid.cursor_row = 4;
        grid.extend_row_selection_to_cursor();
        assert!(!grid.has_row_selection());
    }

    #[test]
    fn copy_current_cell_returns_text() {
        let mut grid = ResultGrid::new();
        grid.set_result(Some(sample_result()));
        grid.cursor_row = 1;
        grid.cursor_col = 1;
        assert_eq!(grid.copy_current_cell().unwrap(), "Bob");
    }

    #[test]
    fn copy_current_cell_without_result_fails() {
        let grid = ResultGrid::new();
        assert!(grid.copy_current_cell().is_err());
    }

    #[test]
    fn copy_current_cell_honors_pending_edit() {
        let mut grid = editable_grid();
        grid.cursor_row = 0;
        grid.cursor_col = 1;
        type_into_cell(&mut grid, "Alicia");
        assert_eq!(grid.copy_current_cell().unwrap(), "Alicia");
    }

    #[test]
    fn cursor_starts_at_end_of_initial_value() {
        let mut grid = editable_grid();
        grid.cursor_row = 0;
        grid.cursor_col = 1; // "Alice"
        grid.begin_edit();
        let edit = grid.editing.as_ref().expect("should be editing");
        assert_eq!(edit.buffer, "Alice");
        assert_eq!(edit.cursor, 5);
    }

    #[test]
    fn cursor_moves_with_arrow_keys() {
        let mut grid = editable_grid();
        grid.cursor_row = 0;
        grid.cursor_col = 1;
        grid.begin_edit();

        grid.handle_edit_key(key(KeyCode::Home));
        assert_eq!(grid.editing.as_ref().unwrap().cursor, 0);

        grid.handle_edit_key(key(KeyCode::Left));
        assert_eq!(
            grid.editing.as_ref().unwrap().cursor,
            0,
            "should clamp at start"
        );

        grid.handle_edit_key(key(KeyCode::Right));
        assert_eq!(grid.editing.as_ref().unwrap().cursor, 1);

        grid.handle_edit_key(key(KeyCode::End));
        assert_eq!(grid.editing.as_ref().unwrap().cursor, 5);

        grid.handle_edit_key(key(KeyCode::Right));
        assert_eq!(
            grid.editing.as_ref().unwrap().cursor,
            5,
            "should clamp at end"
        );
    }

    #[test]
    fn typing_inserts_at_cursor() {
        let mut grid = editable_grid();
        grid.cursor_row = 0;
        grid.cursor_col = 1;
        grid.begin_edit();

        grid.handle_edit_key(key(KeyCode::Home));
        grid.handle_edit_key(key(KeyCode::Char('X')));
        assert_eq!(grid.editing.as_ref().unwrap().buffer, "XAlice");
        assert_eq!(grid.editing.as_ref().unwrap().cursor, 1);

        grid.handle_edit_key(key(KeyCode::End));
        grid.handle_edit_key(key(KeyCode::Char('!')));
        assert_eq!(grid.editing.as_ref().unwrap().buffer, "XAlice!");
        assert_eq!(grid.editing.as_ref().unwrap().cursor, 7);
    }

    #[test]
    fn backspace_deletes_before_cursor() {
        let mut grid = editable_grid();
        grid.cursor_row = 0;
        grid.cursor_col = 1;
        grid.begin_edit();

        // "Alice", move cursor to position 4 (before 'e'), backspace -> "Alie"
        grid.handle_edit_key(key(KeyCode::End));
        grid.handle_edit_key(key(KeyCode::Left));
        assert_eq!(grid.editing.as_ref().unwrap().cursor, 4);

        grid.handle_edit_key(key(KeyCode::Backspace));
        assert_eq!(grid.editing.as_ref().unwrap().buffer, "Alie");
        assert_eq!(grid.editing.as_ref().unwrap().cursor, 3);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut grid = editable_grid();
        grid.cursor_row = 0;
        grid.cursor_col = 1;
        grid.begin_edit();

        grid.handle_edit_key(key(KeyCode::Home));
        grid.handle_edit_key(key(KeyCode::Backspace));
        assert_eq!(grid.editing.as_ref().unwrap().buffer, "Alice");
        assert_eq!(grid.editing.as_ref().unwrap().cursor, 0);
    }

    #[test]
    fn cursor_clamps_to_buffer_bounds() {
        let mut grid = editable_grid();
        grid.cursor_row = 0;
        grid.cursor_col = 1;
        grid.begin_edit();

        // Delete the whole buffer from the end, then keep backspacing at start.
        grid.handle_edit_key(key(KeyCode::End));
        for _ in 0..10 {
            grid.handle_edit_key(key(KeyCode::Backspace));
        }
        assert!(grid.editing.as_ref().unwrap().buffer.is_empty());
        assert_eq!(grid.editing.as_ref().unwrap().cursor, 0);

        grid.handle_edit_key(key(KeyCode::Left));
        assert_eq!(grid.editing.as_ref().unwrap().cursor, 0);
    }

    #[test]
    fn render_shows_high_contrast_cursor_at_character() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut grid = editable_grid();
        grid.cursor_row = 0;
        grid.cursor_col = 1;
        grid.begin_edit();
        // Place cursor over 'l' (position 2) in "Alice".
        grid.handle_edit_key(key(KeyCode::Home));
        grid.handle_edit_key(key(KeyCode::Right));
        grid.handle_edit_key(key(KeyCode::Right));

        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();

        let buf = terminal.backend().buffer();
        // The cursor is drawn with the terminal's foreground as background
        // so it is never the same color as the pane background.
        let cursor_cells: Vec<_> = buf
            .content
            .iter()
            .filter(|c| c.fg == Color::Black && c.bg == Color::White)
            .collect();
        assert!(
            !cursor_cells.is_empty(),
            "expected at least one high-contrast cursor cell"
        );
    }

    #[test]
    fn render_shows_high_contrast_block_cursor_at_end() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut grid = editable_grid();
        grid.cursor_row = 0;
        grid.cursor_col = 1;
        grid.begin_edit(); // cursor at end

        terminal
            .draw(|frame| grid.render(frame, frame.area()))
            .unwrap();

        let buf = terminal.backend().buffer();
        let cursor_cells: Vec<_> = buf
            .content
            .iter()
            .filter(|c| c.fg == Color::Black && c.bg == Color::White)
            .collect();
        assert!(
            !cursor_cells.is_empty(),
            "expected a high-contrast block cursor at end of buffer"
        );
    }
}
