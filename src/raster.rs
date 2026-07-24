//! Rasterize a sprite `Frame` to RGBA pixels for the kitty renderer, reusing
//! the same role->color palette as the half-block path so both look identical.

use crate::palette::{StateStyle, Theme, role_color};
use crate::sprite::{Frame, Role};

/// An RGBA pixel buffer, row-major, 4 bytes per pixel.
pub struct Rgba {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

/// Turn `frame` into RGBA at `scale` px per sprite pixel. `flip` mirrors on x
/// (so a pet can face left). Transparent roles get alpha 0.
pub fn rasterize(
    frame: &Frame,
    hue: u16,
    theme: Theme,
    style: StateStyle,
    scale: usize,
    flip: bool,
) -> Rgba {
    let scale = scale.max(1);
    let (w, h) = (frame.w * scale, frame.h * scale);
    let mut px = vec![0u8; w * h * 4];
    for y in 0..frame.h {
        for x in 0..frame.w {
            let sx = if flip { frame.w - 1 - x } else { x };
            let Some(c) = role_color(frame.cells[y * frame.w + sx], hue, theme, style) else {
                continue; // transparent: leave alpha 0
            };
            for dy in 0..scale {
                for dx in 0..scale {
                    let i = ((y * scale + dy) * w + (x * scale + dx)) * 4;
                    px[i] = c.0;
                    px[i + 1] = c.1;
                    px[i + 2] = c.2;
                    px[i + 3] = 255;
                }
            }
        }
    }
    Rgba { w, h, px }
}

/// Embed `frame` in a larger, fully-transparent canvas padded by `pad` sprite
/// pixels on every side. Used by the kitty backend so a motion offset can be
/// animated by panning a same-size crop window over this bigger, once-
/// transmitted image instead of retransmitting a shifted image every frame.
pub fn pad_frame(frame: &Frame, pad: usize) -> Frame {
    let (w, h) = (frame.w + pad * 2, frame.h + pad * 2);
    let mut cells = vec![Role::Transparent; w * h];
    for y in 0..frame.h {
        for x in 0..frame.w {
            cells[(y + pad) * w + (x + pad)] = frame.cells[y * frame.w + x];
        }
    }
    Frame { w, h, cells }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{StateStyle, Theme};
    use crate::sprite::parse_species;

    const BLOB: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/sprites/test-blob.sprite"
    ));

    fn frame0() -> crate::sprite::Frame {
        let sp = parse_species(BLOB).unwrap();
        // The Idle frame is left-right symmetric (`.MM./MMMM/M##M/.MM.`), so a
        // no-op flip would still satisfy flip_mirrors_on_x — a tautology.
        // Working's frame is asymmetric (`MM../MMM./M##./.MM.`), which makes
        // the flip assertion load-bearing.
        sp.states[&crate::agent::AgentStatus::Working].frames[0].clone()
    }

    #[test]
    fn dimensions_scale_and_rgba_len_is_consistent() {
        let f = frame0();
        let r = rasterize(&f, 120, Theme::Dark, StateStyle::none(), 4, false);
        assert_eq!((r.w, r.h), (f.w * 4, f.h * 4));
        assert_eq!(r.px.len(), r.w * r.h * 4);
    }

    #[test]
    fn transparent_pixels_have_zero_alpha() {
        // test-blob's frame has at least one transparent '.' cell; find a
        // scaled pixel whose alpha is 0.
        let f = frame0();
        let r = rasterize(&f, 120, Theme::Dark, StateStyle::none(), 1, false);
        assert!(
            r.px.chunks(4).any(|p| p[3] == 0),
            "some pixel is transparent"
        );
        assert!(r.px.chunks(4).any(|p| p[3] == 255), "some pixel is opaque");
    }

    #[test]
    fn pad_frame_grows_by_pad_on_every_side_and_keeps_content_centered() {
        let f = frame0();
        let padded = pad_frame(&f, 2);
        assert_eq!((padded.w, padded.h), (f.w + 4, f.h + 4));
        // The original top-left cell now lives at (2, 2) in the padded frame.
        assert_eq!(
            padded.cells[2 * padded.w + 2],
            f.cells[0],
            "original content is embedded at the pad offset"
        );
        // A corner of the new canvas is padding: must be transparent.
        assert_eq!(padded.cells[0], Role::Transparent);
    }

    #[test]
    fn flip_mirrors_on_x() {
        let f = frame0();
        let a = rasterize(&f, 120, Theme::Dark, StateStyle::none(), 1, false);
        let b = rasterize(&f, 120, Theme::Dark, StateStyle::none(), 1, true);
        // top-left of flipped == top-right of unflipped
        let tl_b = &b.px[0..4];
        let tr_a = &a.px[((a.w - 1) * 4)..((a.w - 1) * 4 + 4)];
        assert_eq!(tl_b, tr_a);
    }
}
