//! Overlay icons: tiny pixel-art glyphs (sleep/alert/question) rendered as
//! their own transparent-background images, no bubble/badge chrome. Only the
//! kitty backend draws these today (see `src/kitty_render.rs`); the
//! half-block backend still draws the raw overlay glyph as a text `Span`.

use crate::anim::{OverlayColor, Rgb};
use crate::chrome;
use crate::palette::Theme;
use crate::raster::Rgba;

const ACCENT: Rgb = chrome::ACCENT;

/// Which pixel-art icon a state's overlay glyph maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconKind {
    Sleep,
    Alert,
    Question,
}

impl IconKind {
    /// Map a parsed overlay glyph (`"Zz"`, `"!"`, `"?"`) to its icon. `None`
    /// for anything unrecognized, so an unfamiliar glyph degrades to no icon
    /// rather than a guess.
    pub fn from_glyph(glyph: &str) -> Option<Self> {
        match glyph {
            "Zz" => Some(IconKind::Sleep),
            "!" => Some(IconKind::Alert),
            "?" => Some(IconKind::Question),
            _ => None,
        }
    }

    /// This icon's pixel bitmap, row-major: `#` = ink, `.` = transparent.
    pub fn bitmap(self) -> &'static [&'static str] {
        match self {
            IconKind::Sleep => &SLEEP,
            IconKind::Alert => &ALERT,
            IconKind::Question => &QUESTION,
        }
    }

    /// This icon's ink color. The sprite's own `overlay.color` wins whenever
    /// it specifies one (`done`'s accent-colored `!` vs. `blocked`'s red
    /// `!` — same glyph/icon, different semantics, exactly like the
    /// half-block renderer's badges); `OverlayColor::Default` falls back to a
    /// per-kind default (`Alert` red as a status signal, `Sleep`/`Question`
    /// theme-aware neutral ink so they stay legible on either ground).
    pub fn color(self, theme: Theme, overlay_color: OverlayColor) -> Rgb {
        match overlay_color {
            OverlayColor::Literal(rgb) => rgb,
            OverlayColor::Accent => ACCENT,
            OverlayColor::Default => match self {
                IconKind::Alert => Rgb(0xe6, 0x2d, 0x23),
                IconKind::Sleep | IconKind::Question => match theme {
                    Theme::Dark => Rgb(226, 230, 235),
                    Theme::Light => Rgb(45, 50, 58),
                },
            },
        }
    }
}

const SLEEP: [&str; 5] = ["#####", "...#.", "..#..", ".#...", "#####"];

const ALERT: [&str; 7] = [".#.", ".#.", ".#.", ".#.", "...", ".#.", "..."];

const QUESTION: [&str; 7] = [
    ".###.", "#...#", "....#", "...#.", "..#..", ".....", "..#..",
];

/// Rasterize `kind`'s bitmap to RGBA at `scale` px per icon pixel, padded by
/// `pad` transparent icon-pixels on every side (room for `icon_wave_offset`'s
/// float to shift within without ever revealing a hard edge).
pub fn rasterize_icon(
    kind: IconKind,
    theme: Theme,
    overlay_color: OverlayColor,
    scale: usize,
    pad: usize,
) -> Rgba {
    let bitmap = kind.bitmap();
    let color = kind.color(theme, overlay_color);
    let scale = scale.max(1);
    let (bw, bh) = (bitmap[0].chars().count(), bitmap.len());
    let (w, h) = ((bw + pad * 2) * scale, (bh + pad * 2) * scale);
    let mut px = vec![0u8; w * h * 4];
    for (y, row) in bitmap.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            if ch != '#' {
                continue; // transparent: leave alpha 0
            }
            let (px_x, px_y) = ((x + pad) * scale, (y + pad) * scale);
            for dy in 0..scale {
                for dx in 0..scale {
                    let i = ((px_y + dy) * w + (px_x + dx)) * 4;
                    px[i] = color.0;
                    px[i + 1] = color.1;
                    px[i + 2] = color.2;
                    px[i + 3] = 255;
                }
            }
        }
    }
    Rgba { w, h, px }
}

/// This icon's unpadded (width, height) in icon-pixels.
pub fn icon_size(kind: IconKind) -> (usize, usize) {
    let bitmap = kind.bitmap();
    (bitmap[0].chars().count(), bitmap.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_glyph_recognizes_the_three_overlay_glyphs() {
        assert_eq!(IconKind::from_glyph("Zz"), Some(IconKind::Sleep));
        assert_eq!(IconKind::from_glyph("!"), Some(IconKind::Alert));
        assert_eq!(IconKind::from_glyph("?"), Some(IconKind::Question));
        assert_eq!(IconKind::from_glyph("nope"), None);
    }

    #[test]
    fn bitmaps_are_rectangular() {
        for kind in [IconKind::Sleep, IconKind::Alert, IconKind::Question] {
            let bitmap = kind.bitmap();
            let w = bitmap[0].chars().count();
            assert!(
                bitmap.iter().all(|row| row.chars().count() == w),
                "{kind:?} bitmap rows must all share one width"
            );
        }
    }

    #[test]
    fn alert_defaults_to_red_regardless_of_theme() {
        assert_eq!(
            IconKind::Alert.color(Theme::Dark, OverlayColor::Default),
            IconKind::Alert.color(Theme::Light, OverlayColor::Default)
        );
    }

    #[test]
    fn sleep_and_question_flip_default_ink_with_theme() {
        assert_ne!(
            IconKind::Sleep.color(Theme::Dark, OverlayColor::Default),
            IconKind::Sleep.color(Theme::Light, OverlayColor::Default)
        );
    }

    #[test]
    fn overlay_color_overrides_the_per_kind_default() {
        // `done` and `blocked` share the `Alert` icon (both use `!`) but must
        // read as visually distinct: accent (non-alarming) vs. literal red.
        assert_eq!(
            IconKind::Alert.color(Theme::Dark, OverlayColor::Accent),
            ACCENT
        );
        assert_ne!(
            IconKind::Alert.color(Theme::Dark, OverlayColor::Accent),
            IconKind::Alert.color(Theme::Dark, OverlayColor::Literal(Rgb(0xe6, 0x2d, 0x23)))
        );
    }

    #[test]
    fn rasterize_icon_pads_every_side_with_transparent_margin() {
        let (bw, bh) = icon_size(IconKind::Alert);
        let r = rasterize_icon(IconKind::Alert, Theme::Dark, OverlayColor::Default, 2, 3);
        assert_eq!(r.w, (bw + 6) * 2);
        assert_eq!(r.h, (bh + 6) * 2);
        // Top-left corner sits in the padding: must be transparent.
        assert_eq!(r.px[3], 0, "padded margin is transparent");
    }

    #[test]
    fn rasterize_icon_paints_ink_pixels_opaque() {
        let r = rasterize_icon(IconKind::Alert, Theme::Dark, OverlayColor::Default, 1, 0);
        assert!(
            r.px.chunks(4).any(|p| p[3] == 255),
            "at least one opaque ink pixel"
        );
    }
}
