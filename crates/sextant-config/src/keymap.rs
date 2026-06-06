//! Loading of user key bindings from `keys.toml`.
//!
//! The file is a list of bindings, each mapping a key chord to a named action:
//!
//! ```toml
//! [[binding]]
//! keys = "<Space>e"
//! action = "toggle_editor"
//! ```
//!
//! This crate only parses the raw `(keys, action)` pairs; the UI layer owns the
//! action vocabulary and merges these over its hardcoded defaults. An unknown
//! action name is ignored by the UI, not rejected here.

use serde::Deserialize;

/// A raw key binding as written in `keys.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RawBinding {
    /// The key chord, e.g. `"<Space>e"`, `"<C-s>"`, `"gg"`.
    pub keys: String,
    /// The action name, e.g. `"toggle_editor"`.
    pub action: String,
}

#[derive(Debug, Default, Deserialize)]
struct KeysFile {
    #[serde(default)]
    binding: Vec<RawBinding>,
}

/// Load user key bindings from `~/.config/sextant/keys.toml`.
///
/// Returns an empty list if the file is absent or malformed — the UI falls back
/// entirely to its defaults in that case.
pub fn load_keybindings() -> Vec<RawBinding> {
    let path = super::config_dir().join("keys.toml");
    load_keybindings_from(path)
}

/// Load user key bindings from an explicit path (used by tests).
pub fn load_keybindings_from(path: impl AsRef<std::path::Path>) -> Vec<RawBinding> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|c| toml::from_str::<KeysFile>(&c).ok())
        .map(|f| f.binding)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn parses_bindings() {
        let f = write(
            r#"
[[binding]]
keys = "<Space>e"
action = "toggle_editor"

[[binding]]
keys = "<C-s>"
action = "commit"
"#,
        );
        let bindings = load_keybindings_from(f.path());
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].keys, "<Space>e");
        assert_eq!(bindings[0].action, "toggle_editor");
        assert_eq!(bindings[1].action, "commit");
    }

    #[test]
    fn missing_file_is_empty() {
        let bindings = load_keybindings_from("/nonexistent/keys.toml");
        assert!(bindings.is_empty());
    }

    #[test]
    fn malformed_file_is_empty() {
        let f = write("this is not = valid toml [[[");
        assert!(load_keybindings_from(f.path()).is_empty());
    }
}
