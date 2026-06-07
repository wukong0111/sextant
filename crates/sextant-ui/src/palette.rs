//! Resolve a [`sextant_config::Theme`] into concrete `ratatui` colors.
//!
//! The config layer keeps colors as string tokens to stay `ratatui`-free; this
//! module turns each role into a [`Color`] once at startup so the render path
//! never re-parses strings.

use ratatui::style::Color;
use sextant_config::Theme;

/// Concrete terminal colors for each semantic role (see [`Theme`]).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: Color,
    pub foreground: Color,
    pub accent: Color,
    pub accent_alt: Color,
    pub error: Color,
    pub success: Color,
    pub muted: Color,
    pub selection_fg: Color,
    pub selection_bg: Color,
}

impl Palette {
    /// Resolve every role of `theme` into a terminal color.
    pub fn from_theme(theme: &Theme) -> Self {
        Self {
            background: to_color(&theme.background),
            foreground: to_color(&theme.foreground),
            accent: to_color(&theme.accent),
            accent_alt: to_color(&theme.accent_alt),
            error: to_color(&theme.error),
            success: to_color(&theme.success),
            muted: to_color(&theme.muted),
            selection_fg: to_color(&theme.selection_fg),
            selection_bg: to_color(&theme.selection_bg),
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::from_theme(&Theme::dark())
    }
}

/// Resolve a single color token: a `#rrggbb` hex triplet or a named color.
///
/// Unknown tokens fall back to `Color::Reset` (the terminal default).
fn to_color(token: &str) -> Color {
    let token = token.trim();
    if let Some(hex) = token.strip_prefix('#') {
        if let Some(rgb) = parse_hex(hex) {
            return Color::Rgb(rgb.0, rgb.1, rgb.2);
        }
    }
    match token.to_ascii_lowercase().as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        _ => Color::Reset,
    }
}

/// Parse a 6-digit hex string (no leading `#`) into an RGB triplet.
fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_named_colors() {
        assert_eq!(to_color("cyan"), Color::Cyan);
        assert_eq!(to_color("DarkGray"), Color::DarkGray);
        assert_eq!(to_color("grey"), Color::Gray);
    }

    #[test]
    fn resolves_hex_colors() {
        assert_eq!(to_color("#00d7ff"), Color::Rgb(0, 215, 255));
        assert_eq!(to_color("#000000"), Color::Rgb(0, 0, 0));
    }

    #[test]
    fn unknown_or_malformed_falls_back_to_reset() {
        assert_eq!(to_color("not-a-color"), Color::Reset);
        assert_eq!(to_color("#fff"), Color::Reset); // wrong length
        assert_eq!(to_color("#gggggg"), Color::Reset); // non-hex
    }

    #[test]
    fn dark_palette_matches_legacy_colors() {
        let p = Palette::default();
        assert_eq!(p.background, Color::Black);
        assert_eq!(p.accent, Color::Cyan);
        assert_eq!(p.accent_alt, Color::Yellow);
        assert_eq!(p.error, Color::Red);
    }
}
