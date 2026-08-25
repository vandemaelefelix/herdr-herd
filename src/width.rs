//! Terminal cell width, char by char. `ratatui::buffer::Buffer::set_stringn`
//! already truncates by cell width internally (it walks graphemes via the
//! same `unicode-width` crate), but the half-block caption and the kitty
//! caption both compute their own right-aligned column *before* handing text
//! to a renderer — the kitty backend writes raw escapes with no buffer to
//! fall back on. Both used to budget and measure in `char` count, which
//! undercounts a wide glyph (most emoji, CJK) by one cell each, so an emoji
//! caption could overrun the strip or collide with the `+N` counter. This
//! module is the single place both backends measure and truncate from, so
//! they agree with each other and with `ratatui`'s own math.
//!
//! Pinned to `unicode-width = "0.2"`, the exact version `ratatui-core`
//! already resolves to — std has no Unicode East Asian Width table, and
//! duplicating one badly would be worse than depending on the crate ratatui
//! already vendors.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// `s`'s width in terminal cells (a plain ASCII char is 1, most emoji and
/// CJK characters are 2).
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// The longest prefix of `s` whose cell width is at most `max_cols`, cut on a
/// char boundary that never splits a wide char in half — a char that would
/// only partially fit is dropped rather than emitted, so the result never
/// exceeds `max_cols`.
pub fn truncate_to_width(s: &str, max_cols: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > max_cols {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_width_counts_ascii_as_one_cell_each() {
        assert_eq!(display_width("herd"), 4);
    }

    #[test]
    fn display_width_counts_a_wide_emoji_as_two_cells() {
        assert_eq!(display_width("🤖"), 2);
        assert_eq!(display_width("🤖x"), 3);
    }

    #[test]
    fn truncate_to_width_keeps_a_string_that_already_fits() {
        assert_eq!(truncate_to_width("herd", 10), "herd");
    }

    #[test]
    fn truncate_to_width_drops_a_wide_char_rather_than_split_it() {
        // "🤖" costs 2 cells; a 1-cell budget can't fit any of it.
        assert_eq!(truncate_to_width("🤖x", 1), "");
        assert_eq!(truncate_to_width("🤖x", 2), "🤖");
        assert_eq!(truncate_to_width("🤖x", 3), "🤖x");
    }

    #[test]
    fn truncate_to_width_never_exceeds_the_budget() {
        for max in 0..6 {
            assert!(display_width(&truncate_to_width("🤖 claude", max)) <= max);
        }
    }
}
