//! Shared chrome colours used by both the half-block and kitty backends.
//!
//! Keeping these in one place means a future tweak cannot drift between the
//! ratatui `Color::Rgb` literals and the kitty SGR `38;2;r;g;b` strings.

use crate::anim::Rgb;
use ratatui::style::Color;

/// Hover caption ochre.
pub const CAPTION_OCHRE: Rgb = Rgb(0xd9, 0xa4, 0x41);

/// Build-marker grey.
pub const MARKER_GRAY: Rgb = Rgb(0x6b, 0x7a, 0x6b);

/// Accent badge (done) — warm, non-alarming.
pub const ACCENT: Rgb = Rgb(0xe6, 0xc8, 0x77);

/// Convert an [`Rgb`] chrome colour into a ratatui [`Color`].
pub const fn cell(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// Foreground SGR parameters for kitty (`38;2;r;g;b`, no CSI / suffix).
pub fn sgr_fg(c: Rgb) -> String {
    format!("38;2;{};{};{}", c.0, c.1, c.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_fg_matches_historical_caption_and_marker_bytes() {
        assert_eq!(sgr_fg(CAPTION_OCHRE), "38;2;217;164;65");
        assert_eq!(sgr_fg(MARKER_GRAY), "38;2;107;122;107");
    }
}