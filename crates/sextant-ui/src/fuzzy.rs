//! A reusable fuzzy-filtered list picker.
//!
//! Backs the command palette (`<Space>:`), the table finder (`<Space>f`) and the
//! file opener (`<Space>o`). Typing edits a query that fuzzy-matches item labels
//! (via `fuzzy-matcher`); the list is re-ranked by score on every keystroke.

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

use crate::keymap::Action;

/// What selecting a fuzzy item does.
pub enum FuzzyAction {
    /// Dispatch a keymap action (command palette).
    Dispatch(Action),
    /// Open the table finder (from the command palette).
    FindTable,
    /// Open the file opener (from the command palette).
    OpenFile,
    /// Browse a table by its tree indices (table finder).
    Browse {
        conn: usize,
        schema: usize,
        table: usize,
    },
    /// Load a `.sql` file into the editor (file opener).
    Load(std::path::PathBuf),
}

/// A single selectable entry.
pub struct FuzzyItem {
    pub label: String,
    pub action: FuzzyAction,
}

/// A fuzzy-filtered list picker over [`FuzzyItem`]s.
pub struct FuzzyPicker {
    pub title: String,
    pub query: String,
    items: Vec<FuzzyItem>,
    /// Indices into `items`, ranked best-first for the current query.
    pub filtered: Vec<usize>,
    pub selected: usize,
    matcher: SkimMatcherV2,
}

impl FuzzyPicker {
    /// Build a picker showing all `items` (unfiltered) initially.
    pub fn new(title: impl Into<String>, items: Vec<FuzzyItem>) -> Self {
        let mut picker = Self {
            title: title.into(),
            query: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
            matcher: SkimMatcherV2::default(),
        };
        picker.refilter();
        picker
    }

    /// Recompute the filtered/ranked indices for the current query.
    fn refilter(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.items.len()).collect();
        } else {
            let mut scored: Vec<(i64, usize)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    self.matcher
                        .fuzzy_match(&item.label, &self.query)
                        .map(|score| (score, i))
                })
                .collect();
            // Highest score first; ties keep input order via the index.
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    /// Append a character to the query and re-filter.
    pub fn push(&mut self, c: char) {
        self.query.push(c);
        self.selected = 0;
        self.refilter();
    }

    /// Remove the last query character and re-filter.
    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
        self.refilter();
    }

    /// Move the selection by `delta`, wrapping within the filtered set.
    pub fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            return;
        }
        let len = self.filtered.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
    }

    /// The label of each currently-visible item, in ranked order.
    pub fn visible_labels(&self) -> Vec<&str> {
        self.filtered
            .iter()
            .map(|&i| self.items[i].label.as_str())
            .collect()
    }

    /// Consume the picker, returning the highlighted item's action (if any).
    pub fn into_selected(self) -> Option<FuzzyAction> {
        let idx = *self.filtered.get(self.selected)?;
        self.items.into_iter().nth(idx).map(|i| i.action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<FuzzyItem> {
        ["users", "user_roles", "orders", "products"]
            .into_iter()
            .map(|n| FuzzyItem {
                label: n.to_string(),
                action: FuzzyAction::Load(n.into()),
            })
            .collect()
    }

    #[test]
    fn empty_query_shows_all_in_order() {
        let p = FuzzyPicker::new("t", items());
        assert_eq!(
            p.visible_labels(),
            vec!["users", "user_roles", "orders", "products"]
        );
    }

    #[test]
    fn filters_and_ranks_by_query() {
        let mut p = FuzzyPicker::new("t", items());
        for c in "user".chars() {
            p.push(c);
        }
        let labels = p.visible_labels();
        assert!(labels.contains(&"users"));
        assert!(labels.contains(&"user_roles"));
        assert!(!labels.contains(&"orders"));
        assert!(!labels.contains(&"products"));
    }

    #[test]
    fn no_match_yields_empty() {
        let mut p = FuzzyPicker::new("t", items());
        for c in "zzz".chars() {
            p.push(c);
        }
        assert!(p.visible_labels().is_empty());
        // Selecting nothing is safe.
        assert!(p.into_selected().is_none());
    }

    #[test]
    fn selection_wraps_within_filtered() {
        let mut p = FuzzyPicker::new("t", items());
        assert_eq!(p.selected, 0);
        p.move_selection(-1);
        assert_eq!(p.selected, 3); // wrapped to last
        p.move_selection(1);
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn backspace_restores_matches() {
        let mut p = FuzzyPicker::new("t", items());
        for c in "zzz".chars() {
            p.push(c);
        }
        assert!(p.visible_labels().is_empty());
        for _ in 0..3 {
            p.backspace();
        }
        assert_eq!(p.visible_labels().len(), 4);
    }
}
