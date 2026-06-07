//! Color theme loading: built-in `dark`/`light` plus user overrides.
//!
//! A [`Theme`] is a flat set of semantic *roles* (accent, error, …), each
//! holding a color **token** as a string. Tokens are resolved into terminal
//! colors by the UI layer, so this crate stays free of any `ratatui`
//! dependency. A token is either a named color (`"cyan"`, `"darkgray"`) or a
//! hex triplet (`"#ff8800"`).
//!
//! Resolution order in [`load_theme_from`]:
//! 1. start from the built-in theme named by `[theme] name` (default `dark`;
//!    an unknown name is loaded from `themes/<name>.toml`);
//! 2. apply any per-role overrides given inline under `[theme]`.

use std::path::Path;

use serde::Deserialize;

/// A color theme: one color token per semantic role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Primary background (status bar, modals, panes).
    pub background: String,
    /// Default foreground text.
    pub foreground: String,
    /// Primary highlight: Normal mode, selection, borders.
    pub accent: String,
    /// Secondary highlight: Insert mode, warnings, active transaction.
    pub accent_alt: String,
    /// Errors and destructive markers.
    pub error: String,
    /// Success notices.
    pub success: String,
    /// Dimmed text: hints, inactive items.
    pub muted: String,
    /// Foreground of the selected row/cell.
    pub selection_fg: String,
    /// Background of the selected row/cell.
    pub selection_bg: String,
}

impl Theme {
    /// The built-in dark theme (the historical default look).
    pub fn dark() -> Self {
        Self {
            background: "black".into(),
            foreground: "white".into(),
            accent: "cyan".into(),
            accent_alt: "yellow".into(),
            error: "red".into(),
            success: "green".into(),
            muted: "darkgray".into(),
            selection_fg: "black".into(),
            selection_bg: "cyan".into(),
        }
    }

    /// The built-in light theme.
    pub fn light() -> Self {
        Self {
            background: "white".into(),
            foreground: "black".into(),
            accent: "blue".into(),
            accent_alt: "magenta".into(),
            error: "red".into(),
            success: "green".into(),
            muted: "gray".into(),
            selection_fg: "white".into(),
            selection_bg: "blue".into(),
        }
    }

    /// Resolve a built-in theme by name, falling back to `dark`.
    fn builtin(name: &str) -> Self {
        match name {
            "light" => Self::light(),
            _ => Self::dark(),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// The `config.toml` shape we care about for theming.
#[derive(Deserialize, Default)]
struct ConfigFile {
    theme: Option<ThemeSection>,
}

/// The `[theme]` table: an optional base `name` plus per-role overrides.
#[derive(Deserialize, Default)]
struct ThemeSection {
    name: Option<String>,
    background: Option<String>,
    foreground: Option<String>,
    accent: Option<String>,
    accent_alt: Option<String>,
    error: Option<String>,
    success: Option<String>,
    muted: Option<String>,
    selection_fg: Option<String>,
    selection_bg: Option<String>,
}

impl ThemeSection {
    /// Overlay any set roles onto `theme`.
    fn apply_to(&self, theme: &mut Theme) {
        macro_rules! set {
            ($field:ident) => {
                if let Some(v) = &self.$field {
                    theme.$field = v.clone();
                }
            };
        }
        set!(background);
        set!(foreground);
        set!(accent);
        set!(accent_alt);
        set!(error);
        set!(success);
        set!(muted);
        set!(selection_fg);
        set!(selection_bg);
    }
}

/// Load the theme from `~/.config/sextant/config.toml`, falling back to the
/// built-in dark theme when the file or `[theme]` table is absent or invalid.
pub fn load_theme() -> Theme {
    let config = super::config_dir().join("config.toml");
    let themes = super::themes_dir();
    load_theme_from(&config, &themes)
}

/// Load a theme given an explicit `config.toml` path and a themes directory
/// (used directly by tests).
pub fn load_theme_from(config_path: &Path, themes_dir: &Path) -> Theme {
    let section = std::fs::read_to_string(config_path)
        .ok()
        .and_then(|c| toml::from_str::<ConfigFile>(&c).ok())
        .and_then(|f| f.theme)
        .unwrap_or_default();

    let name = section.name.as_deref().unwrap_or("dark");
    let mut theme = match name {
        "dark" | "light" => Theme::builtin(name),
        custom => load_custom(themes_dir, custom).unwrap_or_default(),
    };
    section.apply_to(&mut theme);
    theme
}

/// Load a custom theme from `<themes_dir>/<name>.toml`: a flat table of role
/// overrides applied over the dark base. Returns `None` if the file is missing
/// or malformed.
fn load_custom(themes_dir: &Path, name: &str) -> Option<Theme> {
    let path = themes_dir.join(format!("{name}.toml"));
    let content = std::fs::read_to_string(path).ok()?;
    let section: ThemeSection = toml::from_str(&content).ok()?;
    let mut theme = Theme::dark();
    section.apply_to(&mut theme);
    Some(theme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn missing_config_yields_dark() {
        let dir = tempfile::tempdir().unwrap();
        let theme = load_theme_from(&dir.path().join("nope.toml"), dir.path());
        assert_eq!(theme, Theme::dark());
    }

    #[test]
    fn selects_light_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = write(dir.path(), "config.toml", "[theme]\nname = \"light\"\n");
        let theme = load_theme_from(&cfg, dir.path());
        assert_eq!(theme, Theme::light());
    }

    #[test]
    fn inline_overrides_win_over_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = write(
            dir.path(),
            "config.toml",
            "[theme]\nname = \"dark\"\naccent = \"#00d7ff\"\nerror = \"magenta\"\n",
        );
        let theme = load_theme_from(&cfg, dir.path());
        assert_eq!(theme.accent, "#00d7ff");
        assert_eq!(theme.error, "magenta");
        // Untouched roles keep the dark base.
        assert_eq!(theme.background, Theme::dark().background);
    }

    #[test]
    fn custom_theme_from_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "solarized.toml", "accent = \"#268bd2\"\n");
        let cfg = write(dir.path(), "config.toml", "[theme]\nname = \"solarized\"\n");
        let theme = load_theme_from(&cfg, dir.path());
        assert_eq!(theme.accent, "#268bd2");
        // Roles not in the custom file fall back to dark.
        assert_eq!(theme.foreground, Theme::dark().foreground);
    }

    #[test]
    fn unknown_custom_without_file_falls_back_to_dark() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = write(dir.path(), "config.toml", "[theme]\nname = \"ghost\"\n");
        let theme = load_theme_from(&cfg, dir.path());
        assert_eq!(theme, Theme::dark());
    }
}
