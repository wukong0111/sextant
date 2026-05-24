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
