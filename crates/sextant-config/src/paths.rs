//! XDG-compliant path resolution.
//!
//! Uses `$XDG_CONFIG_HOME/sextant/` if the environment variable is set,
//! otherwise falls back to `~/.config/sextant/` to match the project
//! specification on both Linux and macOS.

use std::path::PathBuf;

/// Returns the sextant configuration directory.
pub fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("sextant")
    } else {
        dirs::home_dir()
            .expect("home directory must be available")
            .join(".config")
            .join("sextant")
    }
}

/// Returns the directory where saved `.sql` queries live
/// (`$XDG_DATA_HOME/sextant/queries` or `~/.local/share/sextant/queries`).
pub fn queries_dir() -> PathBuf {
    data_dir().join("queries")
}

/// Returns the sextant data directory
/// (`$XDG_DATA_HOME/sextant` or `~/.local/share/sextant`).
fn data_dir() -> PathBuf {
    let data = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg)
    } else {
        dirs::home_dir()
            .expect("home directory must be available")
            .join(".local")
            .join("share")
    };
    data.join("sextant")
}

/// Returns the path to the local application state database
/// (`$XDG_DATA_HOME/sextant/state.db` or `~/.local/share/sextant/state.db`).
pub fn state_db_path() -> PathBuf {
    data_dir().join("state.db")
}

/// Resolve a user-provided query name to a path inside [`queries_dir`],
/// appending a `.sql` extension when none is present.
pub fn query_path(name: &str) -> PathBuf {
    let mut path = queries_dir().join(name);
    if path.extension().is_none() {
        path.set_extension("sql");
    }
    path
}
