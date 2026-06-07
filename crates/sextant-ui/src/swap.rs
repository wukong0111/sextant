//! Editor swap files for crash recovery.
//!
//! While buffers have unsaved changes, the app periodically writes a single
//! per-session swap file (named by PID) holding every dirty buffer's content
//! and cursor. A clean quit deletes it, so any `.swp` found at startup is an
//! orphan from a crashed session and is offered for recovery.
//!
//! Selection state is not persisted (only the cursor); restoring selection is
//! deferred.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One buffer captured in a swap file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapBuffer {
    /// File the buffer was bound to, if any (so recovery can re-bind it).
    pub path: Option<String>,
    /// Cursor position as `(row, col)`.
    pub cursor: (usize, usize),
    /// The buffer's text.
    pub content: String,
}

/// A swap document: all dirty buffers of one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapDoc {
    pub buffers: Vec<SwapBuffer>,
}

/// The swap file path for the current process (`session-<pid>.swp`).
pub fn session_swap_path() -> PathBuf {
    sextant_config::swap_dir().join(format!("session-{}.swp", std::process::id()))
}

/// Serialize a swap document to pretty JSON.
pub fn serialize(doc: &SwapDoc) -> String {
    serde_json::to_string_pretty(doc).unwrap_or_else(|_| "{\"buffers\":[]}".to_string())
}

/// Parse a swap document from JSON, returning `None` if it is malformed.
pub fn parse(content: &str) -> Option<SwapDoc> {
    serde_json::from_str(content).ok()
}

/// Find recoverable swap files (every `*.swp` in the swap dir except the current
/// session's own file), newest first.
pub fn find_orphans() -> Vec<PathBuf> {
    let dir = sextant_config::swap_dir();
    let own = session_swap_path();
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("swp"))
            .filter(|p| *p != own)
            .collect(),
        Err(_) => Vec::new(),
    };
    // Newest first by modification time, when available.
    entries.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .map(std::cmp::Reverse)
    });
    entries
}

/// Read and parse a swap file.
pub fn read(path: &Path) -> Option<SwapDoc> {
    let content = std::fs::read_to_string(path).ok()?;
    parse(&content)
}

/// Remove a swap file, ignoring "not found".
pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let doc = SwapDoc {
            buffers: vec![
                SwapBuffer {
                    path: Some("/tmp/a.sql".into()),
                    cursor: (3, 7),
                    content: "SELECT 1".into(),
                },
                SwapBuffer {
                    path: None,
                    cursor: (0, 0),
                    content: "".into(),
                },
            ],
        };
        let json = serialize(&doc);
        assert_eq!(parse(&json), Some(doc));
    }

    #[test]
    fn parse_rejects_garbage() {
        assert_eq!(parse("not json"), None);
    }

    #[test]
    fn session_path_is_in_swap_dir() {
        let p = session_swap_path();
        assert!(p.starts_with(sextant_config::swap_dir()));
        assert!(
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("session-")
        );
    }
}
