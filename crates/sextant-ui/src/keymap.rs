//! Remappable Normal-mode key bindings.
//!
//! Incoming key events are normalized into [`KeySpec`]s and matched against a
//! [`Keymap`]: a set of chords (one or two keys) mapping to an [`Action`]. The
//! defaults reproduce the historical hardcoded bindings; user entries from
//! `keys.toml` are layered on top (a user chord replaces any default with the
//! same chord, and can shadow a default action).
//!
//! Editor-internal keys (insert/normal toggle, run, save) and modal keys
//! (confirm/cancel) are *not* part of this map — they are handled where the
//! relevant capture occurs. This map covers the main Normal-mode surface
//! (tree + grid navigation and leader commands).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sextant_config::RawBinding;

/// A high-level command produced by resolving a key chord. The host decides how
/// each action behaves in the current focus (e.g. [`Action::Down`] moves the
/// tree selection or the grid cursor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    FocusNext,
    ToggleSidebar,
    ToggleEditor,
    OpenHistory,
    OpenRecent,
    Export,
    Import,
    Down,
    Up,
    Left,
    Right,
    Top,
    Bottom,
    Activate,
    EditCell,
    AddRow,
    DeleteRow,
    Commit,
    Discard,
    EmitDdl,
    Help,
    CommandPalette,
    FindTable,
    OpenFile,
    Snippets,
    SaveSnippet,
    WidenColumn,
    NarrowColumn,
    AutoFitColumn,
    AutoFitAll,
    EnterVisualMode,
    Copy,
    ToggleRowSelection,
    ExtendRowSelection,
}

impl Action {
    /// Map an action name (as written in `keys.toml`) to an [`Action`].
    fn from_name(name: &str) -> Option<Action> {
        Some(match name {
            "quit" => Action::Quit,
            "focus_next" => Action::FocusNext,
            "toggle_sidebar" => Action::ToggleSidebar,
            "toggle_editor" => Action::ToggleEditor,
            "open_history" => Action::OpenHistory,
            "open_recent" => Action::OpenRecent,
            "export" => Action::Export,
            "import" => Action::Import,
            "down" => Action::Down,
            "up" => Action::Up,
            "left" => Action::Left,
            "right" => Action::Right,
            "top" => Action::Top,
            "bottom" => Action::Bottom,
            "activate" => Action::Activate,
            "edit_cell" => Action::EditCell,
            "add_row" => Action::AddRow,
            "delete_row" => Action::DeleteRow,
            "commit" => Action::Commit,
            "discard" => Action::Discard,
            "emit_ddl" => Action::EmitDdl,
            "help" => Action::Help,
            "command_palette" => Action::CommandPalette,
            "find_table" => Action::FindTable,
            "open_file" => Action::OpenFile,
            "snippets" => Action::Snippets,
            "save_snippet" => Action::SaveSnippet,
            "widen_column" => Action::WidenColumn,
            "narrow_column" => Action::NarrowColumn,
            "auto_fit_column" => Action::AutoFitColumn,
            "auto_fit_all" => Action::AutoFitAll,
            "enter_visual_mode" => Action::EnterVisualMode,
            "copy" => Action::Copy,
            "toggle_row_selection" => Action::ToggleRowSelection,
            "extend_row_selection" => Action::ExtendRowSelection,
            _ => return None,
        })
    }

    /// A short human description, shown in the help overlay.
    pub fn label(&self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::FocusNext => "switch focus (tree/grid)",
            Action::ToggleSidebar => "hide/show sidebar",
            Action::ToggleEditor => "open SQL editor",
            Action::OpenHistory => "query history",
            Action::OpenRecent => "recent files",
            Action::Export => "export result set",
            Action::Import => "import into table",
            Action::Down => "move down",
            Action::Up => "move up",
            Action::Left => "collapse / move left",
            Action::Right => "expand / move right",
            Action::Top => "go to top",
            Action::Bottom => "go to bottom",
            Action::Activate => "connect / browse (tree)",
            Action::EditCell => "edit cell (grid)",
            Action::AddRow => "add row (grid)",
            Action::DeleteRow => "delete row (grid)",
            Action::Commit => "commit edits (grid)",
            Action::Discard => "discard edits (grid)",
            Action::EmitDdl => "emit CREATE TABLE (tree)",
            Action::Help => "this help",
            Action::CommandPalette => "command palette",
            Action::FindTable => "find table",
            Action::OpenFile => "open .sql file",
            Action::Snippets => "insert snippet",
            Action::SaveSnippet => "save buffer as snippet",
            Action::WidenColumn => "widen current column or sidebar",
            Action::NarrowColumn => "narrow current column or sidebar",
            Action::AutoFitColumn => "auto-fit current column",
            Action::AutoFitAll => "auto-fit all columns",
            Action::EnterVisualMode => "enter visual mode (grid selection)",
            Action::Copy => "copy cell or selection",
            Action::ToggleRowSelection => "toggle row selection (grid)",
            Action::ExtendRowSelection => "extend row selection to cursor (grid)",
        }
    }
}

/// A single normalized key press: a key code plus whether Ctrl was held.
///
/// Shift is folded into the character itself (`G` rather than Shift+`g`), so it
/// is not tracked separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    code: KeyCode,
    ctrl: bool,
}

impl KeySpec {
    /// Normalize a crossterm key event into a [`KeySpec`].
    pub fn from_event(key: KeyEvent) -> Self {
        Self {
            code: key.code,
            ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        }
    }

    /// Whether this key is the *leader* (`Space`) — the prefix that opens the
    /// which-key menu, as opposed to ordinary multi-key prefixes (`g`, `d`).
    pub fn is_leader(&self) -> bool {
        self.code == KeyCode::Char(' ') && !self.ctrl
    }

    /// Parse a single key token (`"<C-s>"`, `"<Space>"`, `"<Tab>"`, `"g"`).
    fn parse(token: &str) -> Option<Self> {
        if let Some(inner) = token.strip_prefix('<').and_then(|t| t.strip_suffix('>')) {
            // Ctrl chord: `<C-x>`.
            if let Some(rest) = inner.strip_prefix("C-") {
                let mut chars = rest.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                return Some(Self {
                    code: KeyCode::Char(c.to_ascii_lowercase()),
                    ctrl: true,
                });
            }
            let code = match inner.to_ascii_lowercase().as_str() {
                "space" => KeyCode::Char(' '),
                "tab" => KeyCode::Tab,
                "enter" | "cr" | "return" => KeyCode::Enter,
                "esc" => KeyCode::Esc,
                "bs" | "backspace" => KeyCode::Backspace,
                "up" => KeyCode::Up,
                "down" => KeyCode::Down,
                "left" => KeyCode::Left,
                "right" => KeyCode::Right,
                _ => return None,
            };
            return Some(Self { code, ctrl: false });
        }
        // A bare single character.
        let mut chars = token.chars();
        let c = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        Some(Self {
            code: KeyCode::Char(c),
            ctrl: false,
        })
    }
}

/// Split a chord string (`"<Space>e"`, `"gg"`, `"<C-s>"`) into key tokens.
fn tokenize(chord: &str) -> Option<Vec<KeySpec>> {
    let mut specs = Vec::new();
    let chars: Vec<char> = chord.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            if let Some(end) = chars[i..].iter().position(|&c| c == '>') {
                let end = end + i;
                let token: String = chars[i..=end].iter().collect();
                specs.push(KeySpec::parse(&token)?);
                i = end + 1;
            } else {
                // Unmatched '<' is treated as the literal character.
                specs.push(KeySpec::parse(&chars[i].to_string())?);
                i += 1;
            }
        } else {
            specs.push(KeySpec::parse(&chars[i].to_string())?);
            i += 1;
        }
    }
    if specs.is_empty() { None } else { Some(specs) }
}

/// Format a chord back into its display string (inverse of [`tokenize`]).
fn format_chord(specs: &[KeySpec]) -> String {
    specs.iter().map(format_key).collect()
}

/// Format a single key for display (`<Space>`, `<C-s>`, `g`).
fn format_key(spec: &KeySpec) -> String {
    if spec.ctrl {
        if let KeyCode::Char(c) = spec.code {
            return format!("<C-{c}>");
        }
    }
    match spec.code {
        KeyCode::Char(' ') => "<Space>".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Tab => "<Tab>".to_string(),
        KeyCode::Enter => "<Enter>".to_string(),
        KeyCode::Esc => "<Esc>".to_string(),
        KeyCode::Backspace => "<BS>".to_string(),
        KeyCode::Up => "<Up>".to_string(),
        KeyCode::Down => "<Down>".to_string(),
        KeyCode::Left => "<Left>".to_string(),
        KeyCode::Right => "<Right>".to_string(),
        other => format!("{other:?}"),
    }
}

/// The outcome of feeding the current pending key sequence to a [`Keymap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolve {
    /// The sequence matches a binding exactly.
    Action(Action),
    /// The sequence is a strict prefix of one or more bindings — wait for more.
    Pending,
    /// The sequence matches nothing.
    None,
}

/// A set of chord → action bindings.
pub struct Keymap {
    bindings: Vec<(Vec<KeySpec>, Action)>,
}

impl Keymap {
    /// The built-in default bindings (the historical hardcoded set).
    pub fn defaults() -> Self {
        let raw = [
            ("<C-q>", Action::Quit),
            ("<Tab>", Action::FocusNext),
            ("<Space>t", Action::ToggleSidebar),
            ("<Space>e", Action::ToggleEditor),
            ("<Space>h", Action::OpenHistory),
            ("<Space>r", Action::OpenRecent),
            ("<Space>x", Action::Export),
            ("<Space>i", Action::Import),
            ("j", Action::Down),
            ("k", Action::Up),
            ("h", Action::Left),
            ("l", Action::Right),
            ("gg", Action::Top),
            ("G", Action::Bottom),
            ("<Enter>", Action::Activate),
            ("i", Action::EditCell),
            ("o", Action::AddRow),
            ("dd", Action::DeleteRow),
            ("<C-s>", Action::Commit),
            ("<C-z>", Action::Discard),
            ("D", Action::EmitDdl),
            ("<Space>?", Action::Help),
            ("<Space>:", Action::CommandPalette),
            ("<Space>f", Action::FindTable),
            ("<Space>o", Action::OpenFile),
            ("<Space>s", Action::Snippets),
            ("<Space>S", Action::SaveSnippet),
            (">", Action::WidenColumn),
            ("<", Action::NarrowColumn),
            ("<Space>w", Action::AutoFitColumn),
            ("<Space>W", Action::AutoFitAll),
            ("v", Action::EnterVisualMode),
            ("x", Action::ToggleRowSelection),
            ("X", Action::ExtendRowSelection),
            ("y", Action::Copy),
        ];
        let mut map = Self {
            bindings: Vec::new(),
        };
        for (chord, action) in raw {
            if let Some(specs) = tokenize(chord) {
                map.insert(specs, action);
            }
        }
        map
    }

    /// Defaults overlaid with user bindings from `keys.toml`.
    ///
    /// Unparseable chords and unknown action names are skipped.
    pub fn with_user_bindings(user: &[RawBinding]) -> Self {
        let mut map = Self::defaults();
        for b in user {
            if let (Some(specs), Some(action)) = (tokenize(&b.keys), Action::from_name(&b.action)) {
                map.insert(specs, action);
            }
        }
        map
    }

    /// Insert a binding, replacing any existing binding with the same chord.
    fn insert(&mut self, specs: Vec<KeySpec>, action: Action) {
        if let Some(slot) = self.bindings.iter_mut().find(|(s, _)| *s == specs) {
            slot.1 = action;
        } else {
            self.bindings.push((specs, action));
        }
    }

    /// `(chord display, action description)` for every binding, sorted for a
    /// stable help overlay.
    pub fn describe(&self) -> Vec<(String, &'static str)> {
        let mut out: Vec<(String, &'static str)> = self
            .bindings
            .iter()
            .map(|(specs, a)| (format_chord(specs), a.label()))
            .collect();
        out.sort();
        out
    }

    /// Build the which-key menu for an in-progress chord: the keys that can
    /// continue `prefix`, each paired with the action it would trigger. Sorted
    /// by key for a stable display; `None` if nothing continues `prefix`.
    pub fn continuations(&self, prefix: &[KeySpec]) -> Vec<(String, &'static str)> {
        let mut out: Vec<(String, &'static str)> = Vec::new();
        for (specs, action) in &self.bindings {
            if specs.len() > prefix.len() && specs.starts_with(prefix) {
                let next = format_key(&specs[prefix.len()]);
                if !out.iter().any(|(k, _)| *k == next) {
                    out.push((next, action.label()));
                }
            }
        }
        out.sort();
        out
    }

    /// Resolve a pending key sequence against the map.
    pub fn resolve(&self, seq: &[KeySpec]) -> Resolve {
        if let Some((_, action)) = self.bindings.iter().find(|(s, _)| s == seq) {
            return Resolve::Action(*action);
        }
        let is_prefix = self
            .bindings
            .iter()
            .any(|(s, _)| s.len() > seq.len() && s.starts_with(seq));
        if is_prefix {
            Resolve::Pending
        } else {
            Resolve::None
        }
    }
}

/// Tracks the in-progress chord and resolves complete ones to actions.
#[derive(Default)]
pub struct ChordState {
    pending: Vec<KeySpec>,
}

impl ChordState {
    /// The keys pressed so far in the in-progress chord (empty when idle).
    pub fn pending(&self) -> &[KeySpec] {
        &self.pending
    }

    /// The pending sequence formatted for display (`<Space>`, `g`), or `None`
    /// when no chord is in progress.
    pub fn pending_display(&self) -> Option<String> {
        if self.pending.is_empty() {
            None
        } else {
            Some(format_chord(&self.pending))
        }
    }

    /// Feed one key. Returns `Some(action)` when a chord completes; otherwise
    /// `None` (either waiting for more keys, or the sequence was abandoned).
    ///
    /// On a dead end the oldest keys are dropped and matching is retried from
    /// the next key, so a valid binding that happens to follow a dead prefix
    /// still fires.
    pub fn feed(&mut self, keymap: &Keymap, spec: KeySpec) -> Option<Action> {
        self.pending.push(spec);
        loop {
            match keymap.resolve(&self.pending) {
                Resolve::Action(action) => {
                    self.pending.clear();
                    return Some(action);
                }
                Resolve::Pending => return None,
                Resolve::None => {
                    if self.pending.len() <= 1 {
                        self.pending.clear();
                        return None;
                    }
                    self.pending.remove(0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> KeySpec {
        KeySpec {
            code: KeyCode::Char(c),
            ctrl: false,
        }
    }
    fn ctrl(c: char) -> KeySpec {
        KeySpec {
            code: KeyCode::Char(c),
            ctrl: true,
        }
    }

    #[test]
    fn tokenizes_chords() {
        assert_eq!(tokenize("j"), Some(vec![key('j')]));
        assert_eq!(tokenize("gg"), Some(vec![key('g'), key('g')]));
        assert_eq!(tokenize("<Space>e"), Some(vec![key(' '), key('e')]));
        assert_eq!(tokenize("<C-s>"), Some(vec![ctrl('s')]));
        assert_eq!(
            tokenize("<Tab>"),
            Some(vec![KeySpec {
                code: KeyCode::Tab,
                ctrl: false
            }])
        );
    }

    #[test]
    fn tokenizes_literal_less_than() {
        assert_eq!(tokenize("<"), Some(vec![key('<')]));
        assert_eq!(tokenize("<<"), Some(vec![key('<'), key('<')]));
    }

    #[test]
    fn less_than_resolves_to_narrow_column() {
        let map = Keymap::defaults();
        let mut state = ChordState::default();
        assert_eq!(state.feed(&map, key('<')), Some(Action::NarrowColumn));
    }

    #[test]
    fn single_key_resolves() {
        let map = Keymap::defaults();
        let mut state = ChordState::default();
        assert_eq!(state.feed(&map, key('j')), Some(Action::Down));
    }

    #[test]
    fn two_key_chord_resolves_in_order() {
        let map = Keymap::defaults();
        let mut state = ChordState::default();
        // First `g` is a prefix → pending; second `g` completes `gg`.
        assert_eq!(state.feed(&map, key('g')), None);
        assert_eq!(state.feed(&map, key('g')), Some(Action::Top));
    }

    #[test]
    fn leader_chord_resolves() {
        let map = Keymap::defaults();
        let mut state = ChordState::default();
        assert_eq!(state.feed(&map, key(' ')), None);
        assert_eq!(state.feed(&map, key('e')), Some(Action::ToggleEditor));
    }

    #[test]
    fn leader_t_toggles_sidebar() {
        let map = Keymap::defaults();
        let mut state = ChordState::default();
        assert_eq!(state.feed(&map, key(' ')), None);
        assert_eq!(state.feed(&map, key('t')), Some(Action::ToggleSidebar));
    }

    #[test]
    fn shift_x_extends_row_selection() {
        let map = Keymap::defaults();
        let mut state = ChordState::default();
        assert_eq!(state.feed(&map, key('X')), Some(Action::ExtendRowSelection));
    }

    #[test]
    fn dead_prefix_then_valid_key_recovers() {
        let map = Keymap::defaults();
        let mut state = ChordState::default();
        // `g` starts a prefix; `j` doesn't continue `gg`, but `j` alone is Down.
        assert_eq!(state.feed(&map, key('g')), None);
        assert_eq!(state.feed(&map, key('j')), Some(Action::Down));
    }

    #[test]
    fn unbound_key_yields_nothing() {
        let map = Keymap::defaults();
        let mut state = ChordState::default();
        assert_eq!(state.feed(&map, key('z')), None);
        assert!(state.pending.is_empty());
    }

    #[test]
    fn ctrl_chord_resolves() {
        let map = Keymap::defaults();
        let mut state = ChordState::default();
        assert_eq!(state.feed(&map, ctrl('q')), Some(Action::Quit));
    }

    #[test]
    fn user_binding_overrides_default_chord() {
        let user = vec![RawBinding {
            keys: "j".into(),
            action: "up".into(),
        }];
        let map = Keymap::with_user_bindings(&user);
        let mut state = ChordState::default();
        assert_eq!(state.feed(&map, key('j')), Some(Action::Up));
    }

    #[test]
    fn user_can_add_alternate_chord() {
        let user = vec![RawBinding {
            keys: "<C-d>".into(),
            action: "delete_row".into(),
        }];
        let map = Keymap::with_user_bindings(&user);
        let mut state = ChordState::default();
        assert_eq!(state.feed(&map, ctrl('d')), Some(Action::DeleteRow));
        // The default `dd` still works.
        assert_eq!(state.feed(&map, key('d')), None);
        assert_eq!(state.feed(&map, key('d')), Some(Action::DeleteRow));
    }

    #[test]
    fn formats_chords_for_help() {
        assert_eq!(format_chord(&tokenize("<Space>e").unwrap()), "<Space>e");
        assert_eq!(format_chord(&tokenize("gg").unwrap()), "gg");
        assert_eq!(format_chord(&tokenize("<C-s>").unwrap()), "<C-s>");
        assert_eq!(format_chord(&tokenize("<Tab>").unwrap()), "<Tab>");
    }

    #[test]
    fn describe_includes_help_binding() {
        let map = Keymap::defaults();
        let d = map.describe();
        assert!(d.iter().any(|(c, l)| c == "<Space>?" && *l == "this help"));
        // describe() is sorted for a stable overlay.
        let mut sorted = d.clone();
        sorted.sort();
        assert_eq!(d, sorted);
    }

    #[test]
    fn leader_continuations_list_actions() {
        let map = Keymap::defaults();
        let mut state = ChordState::default();
        assert_eq!(state.feed(&map, key(' ')), None);
        let cont = map.continuations(state.pending());
        assert!(cont.contains(&("e".to_string(), "open SQL editor")));
        assert!(cont.contains(&("h".to_string(), "query history")));
        // Sorted for a stable menu.
        let mut sorted = cont.clone();
        sorted.sort();
        assert_eq!(cont, sorted);
    }

    #[test]
    fn non_leader_prefix_echoes_pending() {
        let map = Keymap::defaults();
        let mut state = ChordState::default();
        // `g` is a prefix of `gg`: pending, not leader, echoed as "g".
        assert_eq!(state.feed(&map, key('g')), None);
        assert_eq!(state.pending_display().as_deref(), Some("g"));
        assert!(!state.pending()[0].is_leader());
    }

    #[test]
    fn pending_display_empty_when_idle() {
        let state = ChordState::default();
        assert_eq!(state.pending_display(), None);
    }

    #[test]
    fn leader_key_is_recognized() {
        let map = Keymap::defaults();
        let mut state = ChordState::default();
        assert_eq!(state.feed(&map, key(' ')), None);
        assert!(state.pending()[0].is_leader());
    }

    #[test]
    fn unknown_action_name_is_skipped() {
        let user = vec![RawBinding {
            keys: "q".into(),
            action: "frobnicate".into(),
        }];
        let map = Keymap::with_user_bindings(&user);
        let mut state = ChordState::default();
        assert_eq!(state.feed(&map, key('q')), None);
    }
}
