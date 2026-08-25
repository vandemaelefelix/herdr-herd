//! Half-block renderer: blit the roaming herd into a pixel buffer, emit it as
//! `▀` cells (fg = top pixel, bg = bottom pixel), then overlay state bubbles/
//! badges and a `+N` counter.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossterm::event::{
    self, Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::terminal::size;
use ratatui::Frame;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::agent::Agent;
use crate::anim::{Overlay, OverlayColor, Rgb};
use crate::herd::{Herd, visible_and_hidden};
use crate::herdr::HerdrCli;
use crate::marker;
use crate::member::{Member, priority};
use crate::motion::{Animated, animate};
use crate::palette::{StateStyle, Theme, role_color};
use crate::sprite::{Frame as SpriteFrame, Role, Species, StateSpec};

/// Rows the focus hat occupies above a member's head, plus the 1px hop/bounce
/// headroom sprites already reserve (see `sprites/*.sprite`, `<= 14` px).
pub(crate) const HAT_H: usize = 3;
/// Columns the focus hat occupies, centered over the head anchor.
pub(crate) const HAT_W: usize = 5;
/// The hat's pixel grid, top row first: `.` transparent, `#` outline, `r` red
/// fill. A simple pointed hat. Symmetric, so facing flip never needs to
/// mirror it.
const HAT_ROWS: [&str; HAT_H] = ["..#..", ".#r#.", "#rrr#"];
pub(crate) const HAT_OUTLINE: Rgb = Rgb(0x20, 0x18, 0x18);
pub(crate) const HAT_FILL: Rgb = Rgb(0xd6, 0x2b, 0x2b);
/// Sinks the hat's bottom row into the head's own top row by this many
/// pixels, rather than stacking the whole hat cleanly above it — picked
/// after visual review: floating a full row above the head read as a
/// separate sticker rather than something worn.
const HAT_OVERLAP: i32 = 1;

/// Milliseconds since the Unix epoch — the same absolute reference on every
/// process on this machine (all `herdr-herd render` panes run server-side, so
/// there's no cross-machine clock-skew concern even under `herdr --remote`).
/// This is what makes `motion::animate` agree across every independent pane.
fn wall_clock_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Height of the member band in pixels. Sprites are 16x14 (see sprites/*.sprite);
/// the band is the sprite height, plus 1px of headroom for the hop/bounce
/// lift, plus [`HAT_H`] rows so the focus hat has room above even the tallest
/// (standing) pose without clipping.
pub const MEMBER_PX_H: usize = 15 + HAT_H;

/// Terminal rows the half-block band needs: [`MEMBER_PX_H`] pixel rows packed
/// two per cell.
const BAND_ROWS: u16 = MEMBER_PX_H.div_ceil(2) as u16;

/// Terminal rows the half-block strip needs end to end to show the whole
/// member with no cropping: the band ([`BAND_ROWS`]) plus one overlay lane
/// row for badges/`+N`/the caption/the build marker (#37).
///
/// This is **not** the shipped default: the kitty backend derives its own
/// band height from whatever pane it is given (`kitty_render::member_rows`),
/// so it needs no extra room, and `Auto` picks kitty wherever the terminal
/// supports it. Raising the shipped default would double the strip's
/// vertical footprint for every kitty user for no benefit to them. A user who
/// is on half-block (no kitty support, or `renderer = "half-block"`) and
/// wants the full band, uncropped, should set `strip_rows` to this value
/// explicitly.
pub const STRIP_ROWS: u16 = BAND_ROWS + 1;

/// A pixel canvas: `w * h` optional colors, row-major. `None` = transparent.
pub struct PixelBuf {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Option<Rgb>>,
}

impl PixelBuf {
    /// A fully-transparent buffer of `w` by `h` pixels.
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![None; w * h],
        }
    }

    /// Set the pixel at `(x, y)`, silently ignoring out-of-bounds writes.
    pub fn set(&mut self, x: i32, y: i32, c: Rgb) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            self.px[y as usize * self.w + x as usize] = Some(c);
        }
    }
}

fn to_color(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// The (row, drawn-column) of `fr`'s topmost non-transparent pixel, scanned in
/// already-flipped drawn-space (`flip` mirrors which source column each drawn
/// column samples, matching the body blit below). This anchors the focus hat
/// above the head — or, for the idle "dozing" pose with no distinct head,
/// above the topmost point of the lying-down lump — without any per-species
/// or per-state table: whichever pixel is highest just is the head. Falls
/// back to top-center for a fully transparent frame (never hit by real
/// sprites, all of which paint something).
pub(crate) fn head_anchor(fr: &SpriteFrame, flip: bool) -> (usize, usize) {
    for y in 0..fr.h {
        let mut lo = None;
        let mut hi = None;
        for x in 0..fr.w {
            let sx = if flip { fr.w - 1 - x } else { x };
            if fr.cells[y * fr.w + sx] != Role::Transparent {
                lo.get_or_insert(x);
                hi = Some(x);
            }
        }
        if let (Some(lo), Some(hi)) = (lo, hi) {
            return (y, (lo + hi) / 2);
        }
    }
    (0, fr.w / 2)
}

/// Blit the focus hat into `buf` above the head anchor `(head_row, head_col)`,
/// at body-blit offset `(ox, oy)` — the same offset the sprite body was drawn
/// at, so the hat inherits the identical motion offset, bottom-anchor, and
/// facing-flip transform and never detaches from the head. `head_row` is in
/// the sprite's own local coordinates (pre-offset); `head_col` is already in
/// drawn (post-flip) space, so the hat is not flipped again.
fn draw_hat(buf: &mut PixelBuf, ox: i32, oy: i32, head_row: usize, head_col: usize) {
    let top = oy + head_row as i32 - HAT_H as i32 + HAT_OVERLAP;
    let left = ox + head_col as i32 - (HAT_W / 2) as i32;
    for (y, row) in HAT_ROWS.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let color = match ch {
                '#' => Some(HAT_OUTLINE),
                'r' => Some(HAT_FILL),
                _ => None,
            };
            if let Some(c) = color {
                buf.set(left + x as i32, top + y as i32, c);
            }
        }
    }
}

/// Composite the focus hat directly onto an already-rasterized RGBA image
/// (from `raster::rasterize`), at the head anchor `(head_row, head_col)` —
/// in the *original, unpadded* frame's local sprite-pixel coordinates —
/// given `pad` sprite-pixels of transparent margin already baked into
/// `rgba` on every side (see `raster::pad_frame`). Mirrors `draw_hat`'s
/// placement math exactly, so the kitty backend's baked-in hat lands at the
/// identical pixel position the half-block renderer already uses —
/// pixel-perfect and pose-independent, unlike a separately-placed image
/// (which can only land at whole-terminal-cell resolution and drifts away
/// from poses like the idle "dozing" lump, whose top rows are transparent
/// padding — see `sprites/sheep.sprite`'s comment on normalising frames).
pub(crate) fn stamp_hat(
    rgba: &mut crate::raster::Rgba,
    scale: usize,
    pad: usize,
    head_row: usize,
    head_col: usize,
) {
    let scale = scale.max(1);
    let top = pad as i32 + head_row as i32 - HAT_H as i32 + HAT_OVERLAP;
    let left = pad as i32 + head_col as i32 - (HAT_W as i32 / 2);
    for (y, row) in HAT_ROWS.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let color = match ch {
                '#' => Some(HAT_OUTLINE),
                'r' => Some(HAT_FILL),
                _ => None,
            };
            let Some(c) = color else { continue };
            let px_row = top + y as i32;
            let px_col = left + x as i32;
            if px_row < 0 || px_col < 0 {
                continue;
            }
            let (base_x, base_y) = (px_col as usize * scale, px_row as usize * scale);
            for dy in 0..scale {
                for dx in 0..scale {
                    let (xx, yy) = (base_x + dx, base_y + dy);
                    if xx >= rgba.w || yy >= rgba.h {
                        continue;
                    }
                    let i = (yy * rgba.w + xx) * 4;
                    rgba.px[i] = c.0;
                    rgba.px[i + 1] = c.1;
                    rgba.px[i + 2] = c.2;
                    rgba.px[i + 3] = 255;
                }
            }
        }
    }
}

/// The two half-block glyphs the blit paints with, hoisted to `&'static str`
/// so the hot loop below writes a borrowed symbol into the cell instead of
/// allocating a `String` per painted cell (#43).
const UPPER_HALF: &str = "▀";
const LOWER_HALF: &str = "▄";

/// Emit the pixel buffer as half-block cells into `area`, left-aligned: each
/// cell packs two pixel rows into one terminal row via `▀` (fg = top pixel,
/// bg = bottom pixel) or `▄` when only the bottom pixel is set. Bottom-aligned
/// vertically: when `area` has fewer rows than the buffer needs, rows are
/// dropped off the *top* (the sprite's headroom) rather than the bottom, so a
/// squeezed pane still shows the feet at the floor instead of cropping them
/// off (#37).
pub fn draw_pixels(frame: &mut Frame, area: Rect, buf: &PixelBuf) {
    let rows = buf.h.div_ceil(2);
    let skip = rows.saturating_sub(area.height as usize);
    for ry in skip..rows {
        let out_y = ry - skip;
        for x in 0..buf.w {
            let top = buf.px[(ry * 2) * buf.w + x];
            let bot = if ry * 2 + 1 < buf.h {
                buf.px[(ry * 2 + 1) * buf.w + x]
            } else {
                None
            };
            let cx = area.x + x as u16;
            let cy = area.y + out_y as u16;
            if cx >= area.right() || cy >= area.bottom() {
                continue;
            }
            let (ch, style) = match (top, bot) {
                (None, None) => continue,
                (Some(t), Some(b)) => {
                    (UPPER_HALF, Style::default().fg(to_color(t)).bg(to_color(b)))
                }
                (Some(t), None) => (UPPER_HALF, Style::default().fg(to_color(t))),
                (None, Some(b)) => (LOWER_HALF, Style::default().fg(to_color(b))),
            };
            // Write the cell directly instead of `set_string`, which allocates
            // (`ch.to_string()`) and re-runs grapheme segmentation for a single
            // known 1-column glyph. `set_symbol` + `set_style` is exactly what
            // `set_string` does per grapheme, so the output is byte-identical
            // — the existing snapshots must not move. `cell_mut` yields `None`
            // outside the buffer, which the bounds check above already excludes.
            if let Some(cell) = frame.buffer_mut().cell_mut((cx, cy)) {
                cell.set_symbol(ch).set_style(style);
            }
        }
    }
}

/// One visible member's fully-resolved draw inputs at a single instant: where
/// it sits in the draw order, which member it is, and the state/frame/animation
/// it draws. Produced by [`visible_members`] so the band blit and each
/// backend's frame signature are built from one walk of the simulation and can
/// never disagree about *what* is on screen, only about how finely each
/// backend quantises it.
pub(crate) struct VisibleMember<'a> {
    /// Draw-order index: 0 is drawn first (furthest back), matching the `z`
    /// the kitty backend stamps onto its placements.
    pub z: usize,
    /// Index into `herd.members`.
    pub index: usize,
    pub member: &'a Member,
    pub state: &'a StateSpec,
    pub frame: &'a SpriteFrame,
    pub animated: Animated,
}

/// The walkable pixel range a member's left edge can occupy in a `strip_w`-wide
/// strip: what `motion::animate`'s `x_fraction` is scaled by. A fraction (not
/// a pixel) is what makes the same agent land in the same relative spot in
/// panes of different widths.
pub(crate) fn band_max_x(species: &[Species], strip_w: usize) -> f32 {
    let member_w = species.first().map(|s| s.size().0).unwrap_or(12);
    (strip_w as f32 - member_w as f32).max(0.0)
}

/// A member's band-space left edge, in pixels: the exact quantity
/// [`build_band`] blits at. Shared with the frame signature so a suppressed
/// repaint can never hide a member that actually moved a pixel.
pub(crate) fn band_ox(a: &Animated, max_x: f32) -> i32 {
    (a.x_fraction * max_x + a.offset.dx).round() as i32
}

/// A member's band-space top edge, in pixels. Bottom-anchored: feet rest on the
/// band floor, and motion (`dy <= 0`) lifts the member up into the headroom
/// above it, so a hop/bounce never clips.
pub(crate) fn band_oy(a: &Animated, frame_h: usize) -> i32 {
    MEMBER_PX_H as i32 - frame_h as i32 + a.offset.dy.round() as i32
}

/// The visible members in draw z-order (lowest priority first, so blocked draws
/// last / on top), each resolved to the species state and sprite frame it draws
/// at `now_ms`, plus the number of members the strip has no room for (the `+N`
/// count). Members whose species, state, or frame can't be resolved are
/// dropped; they draw nothing either way.
pub(crate) fn visible_members<'a>(
    herd: &'a Herd,
    species: &'a [Species],
    strip_w: usize,
    now_ms: u64,
) -> (Vec<VisibleMember<'a>>, usize) {
    let member_w = species.first().map(|s| s.size().0).unwrap_or(12);
    let capacity = (strip_w / (member_w * 3 / 4).max(1)).max(1);
    let (visible, hidden) = visible_and_hidden(&herd.members, capacity);

    // z-order: lowest priority first so blocked draws last (on top).
    let mut order = visible;
    order.sort_by_key(|&i| priority(herd.members[i].status));

    let mut out = Vec::with_capacity(order.len());
    for (z, &index) in order.iter().enumerate() {
        let member = &herd.members[index];
        let Some(sp) = species
            .get(member.identity.species_index)
            .or_else(|| species.first())
        else {
            continue;
        };
        let Some(state) = sp.states.get(&member.status) else {
            continue;
        };
        let animated = animate(
            &member.terminal_id,
            member.status,
            state,
            now_ms,
            member.anchor,
        );
        let Some(frame) = state.frames.get(animated.frame_index) else {
            continue;
        };
        out.push(VisibleMember {
            z,
            index,
            member,
            state,
            frame,
            animated,
        });
    }
    (out, hidden)
}

/// Blit every visible member's body — and, for the focused member, its focus hat —
/// into a fresh pixel buffer, in priority z-order (blocked draws last, i.e. on
/// top). The hat is composited into the same buffer right after its member's
/// body, at the same offset, so it shares the body's full transform (motion
/// offset, bottom-anchor, facing flip) and never detaches during motion.
/// `now_ms` (milliseconds since the Unix epoch, or a frozen value under
/// reduced motion) drives every member's position/pose via `motion::animate` — a
/// pure function, so this is fully deterministic given the same inputs.
/// Returns the buffer plus the visible set's z-order (draw order, lowest
/// priority first) so the caller can reuse it for overlays without
/// recomputing the visible/capacity selection.
fn build_band(
    herd: &Herd,
    species: &[Species],
    strip_w: usize,
    theme: Theme,
    now_ms: u64,
) -> (PixelBuf, Vec<usize>) {
    let mut buf = PixelBuf::new(strip_w, MEMBER_PX_H);
    let max_x = band_max_x(species, strip_w);
    let (visible, _hidden) = visible_members(herd, species, strip_w, now_ms);

    for v in &visible {
        let member = v.member;
        let fr = v.frame;
        let style = StateStyle {
            dim: v.state.dim,
            ghost: v.state.ghost,
        };
        let ox = band_ox(&v.animated, max_x);
        let oy = band_oy(&v.animated, fr.h);
        for y in 0..fr.h {
            for x in 0..fr.w {
                let sx = if v.animated.facing_left {
                    fr.w - 1 - x
                } else {
                    x
                };
                if let Some(c) =
                    role_color(fr.cells[y * fr.w + sx], member.identity.hue, theme, style)
                {
                    buf.set(ox + x as i32, oy + y as i32, c);
                }
            }
        }
        if member.focused {
            let (head_row, head_col) = head_anchor(fr, v.animated.facing_left);
            draw_hat(&mut buf, ox, oy, head_row, head_col);
        }
    }
    (buf, visible.iter().map(|v| v.index).collect())
}

/// Draw the whole strip: visible members in priority z-order (blocked draws
/// last, i.e. on top), their overlays (bubbles/badges), the hovered member's
/// caption, and a `+N` marker for any members the strip has no room for. All of
/// these live in a reserved top lane (`lane_y`); the member band is drawn below
/// it, so nothing in the lane ever covers a member. `now_ms` (milliseconds since
/// the Unix epoch, or a frozen value under reduced motion) drives every member's
/// position/pose via `motion::animate` — a pure function, so this is fully
/// deterministic given the same inputs.
pub fn draw_herd(
    frame: &mut Frame,
    herd: &Herd,
    species: &[Species],
    theme: Theme,
    now_ms: u64,
    hover_label: Option<&str>,
) {
    let area = frame.area();
    let strip_w = area.width as usize;
    let (buf, order) = build_band(herd, species, strip_w, theme, now_ms);

    let member_w = species.first().map(|s| s.size().0).unwrap_or(12);
    let max_x = (strip_w as f32 - member_w as f32).max(0.0);
    let capacity = (strip_w / (member_w * 3 / 4).max(1)).max(1);
    let (_visible, hidden) = visible_and_hidden(&herd.members, capacity);

    // Bottom-align the whole strip so it reads as a slim status line whatever
    // the pane's height (herdr enforces a minimum pane height, so the pane can be
    // taller than the content needs): the member band bottom-aligns to the pane
    // floor, and the icon lane sits just above it. Any extra rows fall at the
    // top, blending with the pane above. The icon lane keeps overlays/`+N`/the
    // caption off the member.
    //
    // `band_top` is derived from `overlay_lane_y` (rather than recomputing
    // `band_rows` here) so the lane and the band agree on where one ends and
    // the other begins: a pane shorter than `STRIP_ROWS` shrinks the band
    // instead of overlapping the lane (#37). `draw_pixels` crops a squeezed
    // band from the top (the sprite's headroom), so the feet stay visible at
    // the pane floor.
    let lane_y = overlay_lane_y(area);
    let band_top = lane_y.saturating_add(1);
    let member_area = Rect {
        x: area.x,
        y: band_top,
        width: area.width,
        height: area.bottom().saturating_sub(band_top),
    };
    draw_pixels(frame, member_area, &buf);

    // Overlays (bubbles/badges) as text cells above each visible member.
    for &i in &order {
        let member = &herd.members[i];
        let Some(sp) = species
            .get(member.identity.species_index)
            .or_else(|| species.first())
        else {
            continue;
        };
        let Some(state) = sp.states.get(&member.status) else {
            continue;
        };
        let (glyph, _kind) = match &state.overlay.kind {
            Overlay::Bubble(g) => (g.clone(), 'b'),
            Overlay::Badge(g) => (g.clone(), 'a'),
            Overlay::None => continue,
        };
        let color = match state.overlay.color {
            OverlayColor::Literal(c) => to_color(c),
            OverlayColor::Accent => Color::Rgb(0xe6, 0xc8, 0x77),
            OverlayColor::Default => Color::Gray,
        };
        let animated = animate(
            &member.terminal_id,
            member.status,
            state,
            now_ms,
            member.anchor,
        );
        let cx = area.x
            + ((animated.x_fraction * max_x).round() as u16)
                .saturating_add(3)
                .min(area.width.saturating_sub(glyph.chars().count() as u16));
        frame.buffer_mut().set_span(
            cx,
            lane_y,
            &Span::styled(glyph, Style::default().fg(color)),
            area.width,
        );
    }

    if hidden > 0 {
        let label = format!("+{hidden}");
        let label_w = label.len() as u16;
        let x = area.right().saturating_sub(label_w + 1);
        // In the reserved icon lane (just above the band), right-aligned.
        frame.buffer_mut().set_span(
            x,
            lane_y,
            &Span::styled(label, Style::default().fg(Color::DarkGray)),
            label_w,
        );
    }

    draw_caption(frame, area, lane_y, hover_label, hidden);
}

/// The row of the overlay lane that holds `+N`, the caption, and the build
/// marker: one row above the bottom-aligned member band. The band shrinks
/// below [`BAND_ROWS`] whenever the pane is shorter than [`STRIP_ROWS`] (#37),
/// so the lane always keeps its own row and can never collide with a member.
pub fn overlay_lane_y(area: Rect) -> u16 {
    let band_rows = BAND_ROWS.min(area.height.saturating_sub(1));
    area.bottom().saturating_sub(band_rows).saturating_sub(1)
}

/// Dim gray — the build marker reads as chrome, never competing with the
/// caption's ochre or a member's colors.
const MARKER_GRAY: Color = Color::Rgb(0x6b, 0x7a, 0x6b);

/// Draw the dev build marker at the left of the overlay lane, or nothing in a
/// shipped build (where [`marker::build_marker`] is `None`). The lane's other
/// occupants — the caption and `+N` — are right-aligned, so the two ends never
/// contend for the same columns.
pub fn draw_build_marker(frame: &mut Frame, area: Rect, y: u16) {
    let Some(text) = marker::build_marker() else {
        return;
    };
    if area.height == 0 || area.width == 0 {
        return;
    }
    let text: String = text.chars().take(area.width as usize).collect();
    let w = text.chars().count() as u16;
    frame.buffer_mut().set_span(
        area.x,
        y,
        &Span::styled(text, Style::default().fg(MARKER_GRAY)),
        w,
    );
}

/// Ochre — the hovered member's caption color, distinct from the `+N` marker's
/// neutral dark gray.
const CAPTION_OCHRE: Color = Color::Rgb(0xd9, 0xa4, 0x41);

/// Draw the hover caption top-right in the reserved top lane (row `y`),
/// right-aligned and ochre, or nothing when `label` is `None`. When `hidden`
/// members overflow (the `+N` marker also lives in this lane, further right) the
/// caption stops one column short of it; either way it's truncated so it
/// never overruns the strip width.
pub fn draw_caption(frame: &mut Frame, area: Rect, y: u16, label: Option<&str>, hidden: usize) {
    let Some(label) = label else { return };
    if area.height == 0 || area.width == 0 {
        return;
    }
    // 1-col margin from the strip edge (matching `+N`'s own margin), plus,
    // when `+N` is also shown, its width and a further 1-col gap before it.
    let hidden_w = if hidden > 0 {
        format!("+{hidden}").chars().count() as u16 + 1
    } else {
        0
    };
    let right = area.right().saturating_sub(1 + hidden_w);
    // The dev build marker owns the left of this lane, so the caption's room
    // starts after it. `reserved_cols` is 0 in a shipped build, leaving the
    // shipped layout unchanged.
    let left = area.x.saturating_add(marker::reserved_cols());
    let max_chars = right.saturating_sub(left) as usize;
    if max_chars == 0 {
        return;
    }
    let text: String = label.chars().take(max_chars).collect();
    let w = text.chars().count() as u16;
    let x = right.saturating_sub(w);
    frame.buffer_mut().set_span(
        x,
        y,
        &Span::styled(text, Style::default().fg(CAPTION_OCHRE)),
        w,
    );
}

/// The index of the visible member drawn under terminal column `col`, if any.
/// A mouse column maps 1:1 to a pixel x (half-block cells are one pixel wide).
/// Only members that are actually drawn (the visible set on overflow) are
/// hit-testable; when members overlap, the topmost — highest `priority`, matching
/// the draw z-order — wins. Returns `None` over a gap or out of range. `now_ms`
/// must match whatever was passed to `draw_herd` this frame, so hit-testing
/// agrees with what's actually on screen.
pub fn member_at_column(
    herd: &Herd,
    species: &[Species],
    strip_w: usize,
    col: u16,
    now_ms: u64,
) -> Option<usize> {
    let base_w = species.first().map(|s| s.size().0).unwrap_or(12);
    let max_x = (strip_w as f32 - base_w as f32).max(0.0);
    let capacity = (strip_w / (base_w * 3 / 4).max(1)).max(1);
    let (visible, _hidden) = visible_and_hidden(&herd.members, capacity);

    let x = col as i32;
    let mut best: Option<usize> = None;
    for &i in &visible {
        let member = &herd.members[i];
        let Some(sp) = species
            .get(member.identity.species_index)
            .or_else(|| species.first())
        else {
            continue;
        };
        let Some(state) = sp.states.get(&member.status) else {
            continue;
        };
        let w = sp.size().0 as i32;
        let animated = animate(
            &member.terminal_id,
            member.status,
            state,
            now_ms,
            member.anchor,
        );
        let left = (animated.x_fraction * max_x).round() as i32;
        if x >= left && x < left + w {
            let take = match best {
                None => true,
                Some(b) => priority(member.status) >= priority(herd.members[b].status),
            };
            if take {
                best = Some(i);
            }
        }
    }
    best
}

/// The half-block strip's frame signature: a hash of every input
/// [`draw_herd`] quantises down to a cell, and nothing finer. Sub-pixel motion
/// that rounds away before it reaches [`band_ox`]/[`band_oy`] deliberately
/// does *not* change it. That is the whole point, since an idle member
/// breathes continuously but paints the same pixels for minutes at a time.
///
/// `area` is hashed first because it is the one input whose omission breaks
/// something silently: without it a resize produces the same signature as the
/// frame before it and the strip stops reflowing.
fn band_signature(
    herd: &Herd,
    species: &[Species],
    theme: Theme,
    area: Rect,
    now_ms: u64,
    hover_label: Option<&str>,
) -> u64 {
    let strip_w = area.width as usize;
    let max_x = band_max_x(species, strip_w);
    let (visible, hidden) = visible_members(herd, species, strip_w, now_ms);

    let mut h = DefaultHasher::new();
    (area.x, area.y, area.width, area.height).hash(&mut h);
    theme.hash(&mut h);
    // The `+N` marker and the hover caption are the only other things in the
    // reserved lane that can change between frames (the dev build marker is a
    // compile-time constant).
    hidden.hash(&mut h);
    hover_label.hash(&mut h);
    for v in &visible {
        let member = v.member;
        (
            v.index,
            member.identity.species_index,
            member.identity.hue,
            member.status,
            v.animated.frame_index,
            v.animated.facing_left,
            member.focused,
            band_ox(&v.animated, max_x),
            band_oy(&v.animated, v.frame.h),
            // `draw_herd` places the overlay bubble/badge from the *un-swayed*
            // x, a different rounding from `band_ox`, which folds the sway in.
            // So it needs its own entry, or a sway that cancels a step could
            // freeze a bubble that should have moved a column.
            (v.animated.x_fraction * max_x).round() as i32,
        )
            .hash(&mut h);
    }
    h.finish()
}

/// A pluggable member-strip renderer. The simulation is shared; only drawing and
/// hit-testing differ between backends (half-block vs kitty graphics).
pub trait MemberRenderer {
    /// Draw the whole strip for this frame: the member band, and (where the
    /// backend supports it) overlays/`+N`/the hover caption. `now_ms` drives
    /// every member's position/pose (see `motion::animate`). `hover_label` is the
    /// hovered member's name, if any, drawn top-right in the reserved lane.
    fn draw(
        &mut self,
        frame: &mut Frame,
        herd: &Herd,
        species: &[Species],
        theme: Theme,
        now_ms: u64,
        hover_label: Option<&str>,
    );
    /// A hash of everything this frame would put on screen, at this backend's
    /// own quantisation: the visible members' already-rounded poses and
    /// positions, the `+N` overflow count, the hover caption, and `area`.
    /// `run_loop` skips [`MemberRenderer::draw`] entirely when it matches the
    /// previously drawn frame's: an idle pane produces literally zero changed
    /// cells in ~100% of frames, so the whole frame is pointless work.
    ///
    /// The arguments are exactly `draw`'s, so the rule is simple: the signature
    /// is a hash of the draw's inputs. `area` is one of them; leave it out and
    /// a resize silently stops repainting.
    ///
    /// Cheap by construction: it re-runs `motion::animate` for the visible
    /// members (0.1-0.9 us for a full herd, against a 12-41 us frame) and
    /// allocates nothing per pixel.
    fn frame_signature(
        &self,
        herd: &Herd,
        species: &[Species],
        theme: Theme,
        area: Rect,
        now_ms: u64,
        hover_label: Option<&str>,
    ) -> u64;
    /// The visible member under terminal column `col`, if any (for hover/click).
    /// `now_ms` must match the value passed to `draw` this frame.
    fn member_at_column(
        &self,
        herd: &Herd,
        species: &[Species],
        strip_w: usize,
        col: u16,
        now_ms: u64,
    ) -> Option<usize>;
    /// Release backend resources (kitty: delete transmitted images). Default no-op.
    fn teardown(&mut self) -> io::Result<()> {
        Ok(())
    }
    /// Short backend id (`"half-block"` / `"kitty"`), so selection is
    /// assertable in tests without downcasting.
    fn backend_name(&self) -> &'static str;
}

/// The universal half-block renderer (ratatui `▀▄` cells).
pub struct HalfBlockRenderer;

impl MemberRenderer for HalfBlockRenderer {
    fn draw(
        &mut self,
        frame: &mut Frame,
        herd: &Herd,
        species: &[Species],
        theme: Theme,
        now_ms: u64,
        hover_label: Option<&str>,
    ) {
        draw_herd(frame, herd, species, theme, now_ms, hover_label);
        // Drawn here rather than inside `draw_herd` so the herd itself stays
        // feature-independent: the layout snapshots keep asserting the shipped
        // strip whichever way the crate is built. Mirrors the kitty path, which
        // emits its marker from its own `MemberRenderer::draw`.
        draw_build_marker(frame, frame.area(), overlay_lane_y(frame.area()));
    }
    fn frame_signature(
        &self,
        herd: &Herd,
        species: &[Species],
        theme: Theme,
        area: Rect,
        now_ms: u64,
        hover_label: Option<&str>,
    ) -> u64 {
        band_signature(herd, species, theme, area, now_ms, hover_label)
    }
    fn member_at_column(
        &self,
        herd: &Herd,
        species: &[Species],
        strip_w: usize,
        col: u16,
        now_ms: u64,
    ) -> Option<usize> {
        member_at_column(herd, species, strip_w, col, now_ms)
    }
    fn backend_name(&self) -> &'static str {
        "half-block"
    }
}

/// Choose the backend: forced kinds win; `Auto` probes and falls back to
/// half-blocks when kitty graphics are unavailable (herdr flag off, non-kitty
/// terminal, etc.).
pub fn select_renderer(
    kind: crate::config::RendererKind,
    caps: &mut dyn crate::caps::TerminalCaps,
    scale: usize,
) -> Box<dyn MemberRenderer> {
    use crate::config::RendererKind::*;
    let use_kitty = match kind {
        HalfBlock => false,
        Kitty => true,
        Auto => caps.supports_kitty_graphics(),
    };
    if use_kitty {
        Box::new(crate::kitty_render::KittyRenderer::new(
            scale,
            Box::new(io::stdout()),
        ))
    } else {
        Box::new(HalfBlockRenderer)
    }
}

/// Focus the agent identified by `terminal_id` via `herdr agent focus`.
/// The caller swallows the error — a failed focus must never crash the strip.
pub fn focus_agent(cli: &dyn HerdrCli, terminal_id: &str) -> io::Result<()> {
    cli.run_json(&["agent", "focus", terminal_id]).map(|_| ())
}

/// Where the render loop gets its terminal events. A seam (see the project's
/// testability-seams convention) purely so `run_loop` can be driven from a
/// scripted sequence in tests: `crossterm::event::poll` reads the process's
/// real tty, which a test has no way to feed.
pub trait EventSource {
    /// Wait up to `timeout` for the next terminal event, or `None` if none
    /// arrived. The timeout is what paces the loop at ~12 fps.
    fn poll_event(&mut self, timeout: Duration) -> io::Result<Option<Event>>;
}

/// Production: crossterm's global tty event queue.
pub struct CrosstermEvents;

impl EventSource for CrosstermEvents {
    fn poll_event(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
        if event::poll(timeout)? {
            Ok(Some(event::read()?))
        } else {
            Ok(None)
        }
    }
}

/// Render thread: ~12 fps tick. Drains snapshots, reconciles, steps the herd,
/// draws, handles mouse hover/click, and quits on `q`/Ctrl-C. Restores the
/// terminal (raw mode, alternate screen, mouse capture) on exit.
#[allow(clippy::too_many_arguments)]
pub fn run(
    rx: Receiver<Vec<Agent>>,
    species: Vec<Species>,
    theme: Theme,
    focus: Box<dyn HerdrCli>,
    reduced_motion: bool,
    renderer_kind: crate::config::RendererKind,
    member_scale: usize,
    sound_cfg: crate::config::SoundConfig,
    sound_player: Box<dyn crate::sound::SoundPlayer>,
) -> io::Result<()> {
    // Every terminal mutation below belongs to `guard`, so the `?`s here and a
    // panic inside the loop both hand the terminal back (issue #35).
    crate::term::install_panic_hook();
    let mut guard = crate::term::TerminalGuard::new();
    guard.enter_raw()?;

    // Probe for kitty support BEFORE entering the alternate screen and enabling
    // mouse capture: the query/DA round-trip then happens on the main screen
    // with no mouse-event bytes to wade through, and any reply is fully consumed
    // before rendering starts. Raw mode (enabled above) is all the probe needs.
    let mut caps = crate::caps::RealCaps::new();
    let mut renderer = select_renderer(renderer_kind, &mut caps, member_scale);

    guard.enter_screen()?;
    // A FIXED viewport, not the default fullscreen one. `Terminal::draw`
    // autoresizes a fullscreen viewport, and `crossterm::terminal::size` is a
    // `File::open("/dev/tty")` + ioctl + close every time (~20 us in a real
    // pty, and 15-35 ms via the `tput` fallback when /dev/tty can't be
    // opened). A fixed viewport performs none of them: `run_loop` resizes it
    // from the `Event::Resize` it already receives. This one call is the only
    // size query left, at startup.
    let (cols, rows) = size()?;
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, cols, rows)),
        },
    )?;
    let result = run_loop(
        &mut terminal,
        rx,
        &species,
        theme,
        focus.as_ref(),
        reduced_motion,
        renderer.as_mut(),
        &sound_cfg,
        sound_player.as_ref(),
        &mut CrosstermEvents,
    );

    let _ = renderer.teardown(); // best-effort: deletes any transmitted kitty images
    // The loop's own failure is the interesting one; a restore failure is only
    // reported when the loop succeeded. Restoring used to `?` ahead of this and
    // discard `result` entirely.
    result.and(guard.restore())
}

/// The hover caption for the current cursor position: the hovered member's
/// label when the cursor is over one (`hit`), or `None` over empty strip.
/// Clearing on empty is deliberate — the name vanishes when you're not on a
/// sheep, rather than sticking at the last one shown.
fn next_hover(hit: Option<usize>, members: &[Member]) -> Option<String> {
    hit.map(|i| members[i].label.clone())
}

#[allow(clippy::too_many_arguments)]
fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    rx: Receiver<Vec<Agent>>,
    species: &[Species],
    theme: Theme,
    focus: &dyn HerdrCli,
    reduced_motion: bool,
    renderer: &mut dyn MemberRenderer,
    sound_cfg: &crate::config::SoundConfig,
    sound_player: &dyn crate::sound::SoundPlayer,
    events: &mut dyn EventSource,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    let tick = Duration::from_millis(83); // ~12 fps
    let species_count = species.len().max(1);
    let mut herd = Herd::new();
    let mut hovered: Option<String> = None;
    // The signature of the frame currently on screen. `None` until the first
    // draw, which is why the first frame is never skipped.
    let mut last_sig: Option<u64> = None;
    // The geometry the strip is drawn at. Read once from the viewport here and
    // then kept in step with `Event::Resize` below, rather than re-queried per
    // frame: `terminal.size()` opens /dev/tty on every call, which cost more
    // than rendering the sheep did.
    let mut area = terminal.get_frame().area();
    // One claim store for the whole session: every pane sees every transition,
    // so the sound is claimed once per transition, not once per pane.
    let sound_claim = crate::sound::session_claim();
    loop {
        // Reduced motion freezes every member at one fixed instant (0) instead of
        // the live clock — `motion::animate` is a pure function of this value,
        // so "frozen" falls out for free with no separate code path. Computed
        // up front so `herd.reconcile`'s freeze-anchor capture (which member left
        // Working, and where) uses the same instant this frame draws at.
        let now_ms = if reduced_motion {
            0
        } else {
            wall_clock_now_ms()
        };
        let mut transitions = Vec::new();
        while let Ok(agents) = rx.try_recv() {
            transitions.extend(herd.reconcile(&agents, species_count, now_ms));
        }
        if !transitions.is_empty() {
            crate::sound::play_claimed(sound_player, sound_claim.as_ref(), &transitions, sound_cfg);
        }
        // Mouse hit-testing has to agree with what is on screen, so the width
        // it uses is the width the last frame actually drew at.
        let strip_w = area.width as usize;
        let caption = hovered.clone();
        // Skip the whole frame when nothing visible changed. Measured on an
        // idle pane, 1197 of 1199 frames produce zero changed cells, yet each
        // one still allocates an 8.6-14.4 KB `PixelBuf`, blits it, and writes
        // ratatui's trailer. Note this suppresses an identical *repaint* only:
        // `now_ms` above is still sampled fresh every iteration and
        // `motion::animate` stays pure, so cross-pane animation sync (which
        // depends on independent processes agreeing on absolute wall-clock
        // time) is untouched.
        let sig = renderer.frame_signature(&herd, species, theme, area, now_ms, caption.as_deref());
        if last_sig != Some(sig) {
            terminal.draw(|f| {
                renderer.draw(f, &herd, species, theme, now_ms, caption.as_deref());
            })?;
            last_sig = Some(sig);
        }

        if let Some(ev) = events.poll_event(tick)? {
            match ev {
                Event::Key(k) => {
                    let quit = k.code == KeyCode::Char('q')
                        || (k.code == KeyCode::Char('c')
                            && k.modifiers.contains(KeyModifiers::CONTROL));
                    if quit {
                        return Ok(());
                    }
                }
                Event::Mouse(MouseEvent { kind, column, .. }) => match kind {
                    MouseEventKind::Moved => {
                        // Follow the cursor: show the hovered member's name, and
                        // clear it when the cursor is over empty strip — so the
                        // caption disappears when you're not on a sheep.
                        let hit =
                            renderer.member_at_column(&herd, species, strip_w, column, now_ms);
                        hovered = next_hover(hit, &herd.members);
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        if let Some(i) =
                            renderer.member_at_column(&herd, species, strip_w, column, now_ms)
                        {
                            let tid = herd.members[i].terminal_id.clone();
                            // Swallow focus errors: the strip must keep running.
                            let _ = focus_agent(focus, &tid);
                        }
                    }
                    _ => {}
                },
                // A fixed viewport is never autoresized, so this is what keeps
                // the strip reflowing. `area` feeds the next frame signature,
                // which is what forces the repaint (`terminal.resize` has
                // already cleared the viewport and reset the back buffer, and
                // the kitty backend purges and retransmits its images when it
                // sees the new area).
                Event::Resize(w, h) => {
                    let resized = Rect::new(0, 0, w, h);
                    // Terminals emit a resize for a geometry that did not
                    // actually change; honouring it would clear the viewport
                    // and leave the strip blank until something else moved.
                    if resized != area {
                        terminal.resize(resized)?;
                        area = resized;
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentStatus};
    use crate::herd::Herd;
    use crate::identity::identity_for;
    use crate::member::Member;
    use crate::palette::Theme;
    use crate::sprite::parse_species;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const BLOB: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/sprites/test-blob.sprite"
    ));

    /// An arbitrary fixed instant: snapshot tests just need *some* stable
    /// value, since `motion::animate` is pure (same inputs, same pixels).
    const NOW_MS: u64 = 1_700_000_000_000;

    fn agent(tid: &str, s: AgentStatus) -> Agent {
        Agent {
            agent: None,
            agent_status: s,
            name: None,
            cwd: "/".into(),
            foreground_cwd: "/".into(),
            workspace_id: "w".into(),
            tab_id: "t".into(),
            pane_id: "p".into(),
            terminal_id: tid.into(),
            revision: 0,
            focused: false,
            hover_label: None,
        }
    }

    fn fixed_herd(states: &[AgentStatus]) -> Herd {
        let mut h = Herd::new();
        let agents: Vec<_> = states
            .iter()
            .enumerate()
            .map(|(i, s)| agent(&format!("t{i}"), *s))
            .collect();
        h.reconcile(&agents, 1, NOW_MS);
        h
    }

    #[test]
    fn hover_caption_follows_the_cursor_and_clears_over_empty_strip() {
        let herd = fixed_herd(&[AgentStatus::Idle, AgentStatus::Working]);
        // Over a member: its label is shown.
        assert_eq!(
            next_hover(Some(1), &herd.members),
            Some(herd.members[1].label.clone())
        );
        // Over empty strip: the caption clears. Regression guard — a "sticky"
        // hover that kept the last name here is the bug this restores.
        assert_eq!(
            next_hover(None, &herd.members),
            None,
            "moving off all sheep must hide the name"
        );
    }

    #[test]
    fn draw_pixels_writes_the_half_block_symbol_and_style_into_each_cell() {
        // The blit writes cells directly (`set_symbol` + `set_style`) rather
        // than through `set_string`, which allocated a `String` per painted
        // cell (#43). That is exactly what `set_string` does per grapheme, so
        // the output must stay byte-identical — pinned here per cell, where the
        // strip snapshots only cover it in aggregate.
        use crate::anim::Rgb;
        let mut buf = PixelBuf::new(4, 2);
        buf.set(0, 0, Rgb(1, 2, 3)); // top pixel only
        buf.set(1, 0, Rgb(1, 2, 3)); // top and bottom
        buf.set(1, 1, Rgb(4, 5, 6));
        buf.set(2, 1, Rgb(4, 5, 6)); // bottom pixel only
        // column 3 stays fully transparent
        let mut terminal = Terminal::new(TestBackend::new(4, 1)).unwrap();
        terminal
            .draw(|f| draw_pixels(f, Rect::new(0, 0, 4, 1), &buf))
            .unwrap();
        let rendered = terminal.backend().buffer().clone();
        let cell = |x: u16| rendered.cell((x, 0)).expect("a cell inside the buffer");
        assert_eq!(cell(0).symbol(), "▀");
        assert_eq!(cell(0).fg, Color::Rgb(1, 2, 3));
        assert_eq!(
            cell(0).bg,
            Color::Reset,
            "no bottom pixel leaves the background untouched"
        );
        assert_eq!(cell(1).symbol(), "▀");
        assert_eq!(cell(1).fg, Color::Rgb(1, 2, 3));
        assert_eq!(
            cell(1).bg,
            Color::Rgb(4, 5, 6),
            "the bottom pixel is the bg"
        );
        assert_eq!(cell(2).symbol(), "▄");
        assert_eq!(cell(2).fg, Color::Rgb(4, 5, 6));
        assert_eq!(
            cell(3).symbol(),
            " ",
            "a fully transparent column is skipped, not painted"
        );
    }

    #[test]
    fn renders_each_state_in_the_strip() {
        use AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[Idle, Working, Done, Blocked, Unknown]);
        let mut terminal = Terminal::new(TestBackend::new(90, 11)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn renders_a_hat_above_the_focused_member_and_nothing_above_the_rest() {
        use AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let mut h = Herd::new();
        let agents: Vec<_> = [Working, Idle]
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut a = agent(&format!("t{i}"), *s);
                a.focused = i == 0; // only the first member is focused
                a
            })
            .collect();
        h.reconcile(&agents, 1, NOW_MS);
        let mut terminal = Terminal::new(TestBackend::new(40, 11)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &h, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn renders_overflow_counter() {
        use AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[Idle; 20]);
        let mut terminal = Terminal::new(TestBackend::new(40, 11)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn reconcile_then_draw_shows_the_incoming_herd() {
        // A focused integration check: feed one snapshot, reconcile, draw, snapshot.
        use crate::agent::AgentStatus::*;
        let species = vec![crate::sprite::parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        herd.reconcile(&[agent("a", Working), agent("b", Blocked)], 1, NOW_MS);
        let mut terminal = Terminal::new(TestBackend::new(60, 11)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn a_member_leaving_working_freezes_in_place_instead_of_teleporting_when_drawn() {
        // End-to-end: reconcile captures the anchor on the Working->Idle
        // transition, and draw_herd's own animate() call (via member.anchor)
        // must actually use it — not just the unit-level animate() tests.
        use crate::agent::AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        herd.reconcile(&[agent("settling", Working)], 1, 0);
        herd.reconcile(&[agent("settling", Idle)], 1, 5_000);

        let render_at = |ms: u64| {
            let mut t = Terminal::new(TestBackend::new(40, 10)).unwrap();
            t.draw(|f| draw_herd(f, &herd, &species, Theme::Dark, ms, None))
                .unwrap();
            format!("{}", t.backend())
        };
        // Sampled well after settling; a teleport to the identity rest-x
        // would very likely differ from the anchored frame (and even if it
        // coincidentally matched once, holding steady across two more
        // instants would not).
        let frozen = render_at(5_000);
        assert_eq!(
            frozen,
            render_at(20_000),
            "a frozen member must not drift or teleport as time passes"
        );
        assert_eq!(
            frozen,
            render_at(90_000),
            "still frozen well beyond a full wander period"
        );
    }

    #[test]
    fn member_at_column_returns_the_topmost_member_when_they_overlap() {
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        herd.members.push(Member::new(
            "idle".into(),
            identity_for("idle", 1),
            AgentStatus::Idle,
        ));
        herd.members.push(Member::new(
            "blk".into(),
            identity_for("blk", 1),
            AgentStatus::Blocked,
        ));
        // A strip narrower than 2x the member width (test-blob is 4px wide)
        // guarantees any two members' hit ranges intersect, however their
        // identity-derived rest positions land — no need to know the exact
        // hash values.
        let strip_w = 7usize;
        let max_x = (strip_w as f32 - 4.0).max(0.0);
        let sp = &species[0];
        let idle_left = (animate(
            "idle",
            AgentStatus::Idle,
            &sp.states[&AgentStatus::Idle],
            NOW_MS,
            None,
        )
        .x_fraction
            * max_x)
            .round() as i32;
        let blk_left = (animate(
            "blk",
            AgentStatus::Blocked,
            &sp.states[&AgentStatus::Blocked],
            NOW_MS,
            None,
        )
        .x_fraction
            * max_x)
            .round() as i32;
        let overlap_col = idle_left.max(blk_left) as u16;

        let hit = member_at_column(&herd, &species, strip_w, overlap_col, NOW_MS)
            .expect("a member under the overlap column");
        assert_eq!(
            herd.members[hit].terminal_id, "blk",
            "blocked draws on top, so it wins the hit"
        );
    }

    #[test]
    fn member_at_column_breaks_ties_by_draw_order_topmost_wins() {
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        // Same-priority overlap: "b" is pushed later, so the stable sort in
        // draw_herd keeps it later in z-order and it draws on top. Same
        // narrow-strip trick as above forces the overlap.
        herd.members.push(Member::new(
            "a".into(),
            identity_for("a", 1),
            AgentStatus::Idle,
        ));
        herd.members.push(Member::new(
            "b".into(),
            identity_for("b", 1),
            AgentStatus::Idle,
        ));
        let strip_w = 7usize;
        let max_x = (strip_w as f32 - 4.0).max(0.0);
        let sp = &species[0];
        let idle_state = &sp.states[&AgentStatus::Idle];
        let a_left = (animate("a", AgentStatus::Idle, idle_state, NOW_MS, None).x_fraction * max_x)
            .round() as i32;
        let b_left = (animate("b", AgentStatus::Idle, idle_state, NOW_MS, None).x_fraction * max_x)
            .round() as i32;
        let overlap_col = a_left.max(b_left) as u16;

        let hit = member_at_column(&herd, &species, strip_w, overlap_col, NOW_MS)
            .expect("a member under the overlap column");
        assert_eq!(
            herd.members[hit].terminal_id, "b",
            "later-pushed same-priority member draws on top, so it wins the hit"
        );
    }

    /// Row `y` as one `String` of cell symbols, plus each cell's foreground —
    /// lets a test assert on text and color together from a `Buffer` directly
    /// (`TestBackend`'s `Display` only exposes symbols).
    fn row_text_and_fg(buffer: &ratatui::buffer::Buffer, y: u16) -> (String, Vec<Color>) {
        let text = (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol().chars().next().unwrap_or(' '))
            .collect();
        let fg = (0..buffer.area.width).map(|x| buffer[(x, y)].fg).collect();
        (text, fg)
    }

    #[test]
    fn caption_renders_top_right_in_ochre() {
        // A pane exactly the band's height + 1 lane row, so the lane sits at
        // row 0 (matching `overlays_never_occlude_the_member`'s convention).
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working, AgentStatus::Idle]);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let completed = terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, Some("backend-api")))
            .unwrap();
        let (row0, fg) = row_text_and_fg(completed.buffer, 0);
        assert!(
            row0.trim_end().ends_with("backend-api"),
            "caption is right-aligned in the top lane (row 0): {row0:?}"
        );
        let label_start = row0.find("backend-api").unwrap();
        assert_eq!(
            fg[label_start],
            Color::Rgb(0xd9, 0xa4, 0x41),
            "caption is ochre"
        );
    }

    #[test]
    fn caption_no_longer_reserves_the_bottom_row() {
        // Freeing the bottom row means the member band now bottom-aligns exactly
        // to the pane floor: the last row is member pixels, not blank/caption.
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working, AgentStatus::Idle]);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, Some("backend-api")))
            .unwrap();
        let rows = rows_of(terminal.backend());
        let last = rows.last().unwrap();
        assert!(
            !last.contains("backend-api"),
            "caption no longer occupies the bottom row: {last:?}"
        );
        assert!(
            last.contains('▀') || last.contains('▄'),
            "the band now bottom-aligns to the pane floor: {last:?}"
        );
    }

    #[test]
    fn caption_truncates_and_stays_left_of_the_overflow_marker() {
        // `Working` carries no overlay glyph (unlike `Idle`'s "Zz" bubble),
        // so the top lane holds only the caption and the `+N` marker.
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working; 30]);
        let mut terminal = Terminal::new(TestBackend::new(24, 10)).unwrap();
        terminal
            .draw(|f| {
                draw_herd(
                    f,
                    &herd,
                    &species,
                    Theme::Dark,
                    NOW_MS,
                    Some("a-very-long-agent-name-that-does-not-fit"),
                )
            })
            .unwrap();
        let rows = rows_of(terminal.backend());
        let row0 = &rows[0];
        assert!(row0.contains('+'), "the +N marker is still shown");
        let plus_at = row0.find('+').unwrap();
        let last_caption_char = row0[..plus_at].trim_end().len();
        assert!(
            last_caption_char < plus_at,
            "caption stays left of the +N marker with a gap: {row0:?}"
        );
        assert!(row0.chars().count() <= 24, "never overruns the strip width");
    }

    /// A dev build has to answer "which build is in this pane?" at a glance,
    /// so the marker takes the left of the overlay lane the caption already
    /// shares with `+N` on the right.
    #[test]
    #[cfg(feature = "dev-marker")]
    fn a_dev_build_draws_the_build_marker_at_the_left_of_the_overlay_lane() {
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working]);
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        terminal
            .draw(|f| {
                MemberRenderer::draw(
                    &mut HalfBlockRenderer,
                    f,
                    &herd,
                    &species,
                    Theme::Dark,
                    NOW_MS,
                    None,
                )
            })
            .unwrap();
        let lane = &rows_of(terminal.backend())[0];
        let marker = crate::marker::build_marker().unwrap();
        assert!(
            lane.starts_with(marker),
            "marker should own the left of the lane: {lane:?}"
        );
    }

    #[test]
    #[cfg(feature = "dev-marker")]
    fn a_long_caption_is_truncated_rather_than_overwriting_the_build_marker() {
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working]);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|f| {
                MemberRenderer::draw(
                    &mut HalfBlockRenderer,
                    f,
                    &herd,
                    &species,
                    Theme::Dark,
                    NOW_MS,
                    Some("a-very-long-agent-name-that-does-not-fit"),
                )
            })
            .unwrap();
        let lane = &rows_of(terminal.backend())[0];
        let marker = crate::marker::build_marker().unwrap();
        assert!(
            lane.starts_with(marker),
            "the caption must not eat into the marker: {lane:?}"
        );
    }

    /// Dump a TestBackend as one `String` per terminal row (symbols only).
    fn rows_of<B: std::fmt::Display>(backend: &B) -> Vec<String> {
        format!("{backend}")
            .lines()
            .map(|l| l.trim().trim_matches('"').to_string())
            .collect()
    }

    #[test]
    fn overlays_never_occlude_the_member() {
        // A done member shows a '!' badge. The badge must live in the top lane
        // (row 0) with NO member pixels, and the member must be drawn in the band
        // below (row 1+), so the icon can never cover the animal.
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Done]);
        let mut terminal = Terminal::new(TestBackend::new(30, 10)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        let rows = rows_of(terminal.backend());
        assert!(rows[0].contains('!'), "badge sits in the top lane (row 0)");
        assert!(
            !rows[0].contains('▀') && !rows[0].contains('▄'),
            "the top lane holds no member pixels — nothing to occlude"
        );
        let band_has_member = rows[1..].iter().any(|r| r.contains('▀') || r.contains('▄'));
        assert!(
            band_has_member,
            "the member is drawn in the band below the lane"
        );
    }

    #[test]
    fn overflow_counter_lives_in_the_top_lane() {
        // With many members, +N must render in the reserved top lane (row 0),
        // never mid-band on top of a member.
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Idle; 30]);
        let mut terminal = Terminal::new(TestBackend::new(24, 10)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        let rows = rows_of(terminal.backend());
        assert!(rows[0].contains('+'), "the +N marker is in the top lane");
    }

    #[test]
    fn member_at_column_is_none_over_a_gap() {
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        herd.members.push(Member::new(
            "a".into(),
            identity_for("a", 1),
            AgentStatus::Idle,
        ));
        // A member's occupied range never reaches past `strip_w` itself (its
        // rest fraction is in [0,1] of the walkable width), so the column
        // right at the strip's edge is a gap regardless of the identity hash.
        assert!(
            member_at_column(&herd, &species, 200, 200, NOW_MS).is_none(),
            "column past the strip's edge is empty"
        );
    }

    use crate::herdr::{CommandRunner, LiveHerdr};
    use std::cell::{Cell, RefCell};
    use std::ffi::OsStr;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};
    use std::rc::Rc;

    struct Recorder {
        args: Rc<RefCell<Vec<String>>>,
    }
    impl CommandRunner for Recorder {
        fn run(&self, _program: &OsStr, args: &[&str]) -> std::io::Result<Output> {
            *self.args.borrow_mut() = args.iter().map(|s| s.to_string()).collect();
            Ok(Output {
                status: ExitStatus::from_raw(0),
                stdout: b"{\"result\":{}}".to_vec(),
                stderr: Vec::new(),
            })
        }
    }

    #[test]
    fn left_facing_member_is_mirrored() {
        // A working member's facing flips as it wanders back and forth. Find two
        // instants where the same agent faces opposite ways (motion::animate
        // as an oracle, rather than forcing the field directly — it's derived
        // now, not stored), and assert the rendered band differs between them.
        use crate::agent::AgentStatus::Working;
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[Working]);
        let state = &species[0].states[&Working];
        let right_ms = (0..80_000u64)
            .step_by(97)
            .find(|&ms| !animate("t0", Working, state, ms, None).facing_left)
            .expect("some instant facing right");
        let left_ms = (0..80_000u64)
            .step_by(97)
            .find(|&ms| animate("t0", Working, state, ms, None).facing_left)
            .expect("some instant facing left");
        let render = |ms: u64| {
            let mut t = Terminal::new(TestBackend::new(40, 10)).unwrap();
            t.draw(|f| draw_herd(f, &herd, &species, Theme::Dark, ms, None))
                .unwrap();
            format!("{}", t.backend())
        };
        assert_ne!(
            render(right_ms),
            render(left_ms),
            "mirroring must change the pixels"
        );
    }

    #[test]
    fn focus_agent_shells_agent_focus_with_the_terminal_id() {
        let args = Rc::new(RefCell::new(Vec::new()));
        let cli = LiveHerdr::with_runner(
            "herdr",
            Recorder {
                args: Rc::clone(&args),
            },
        );
        focus_agent(&cli, "term_abc").unwrap();
        assert_eq!(*args.borrow(), vec!["agent", "focus", "term_abc"]);
    }

    #[test]
    fn auto_picks_kitty_when_supported_else_half_block() {
        use crate::caps::FakeCaps;
        use crate::config::RendererKind;
        let is_kitty = |r: &dyn MemberRenderer| r.backend_name() == "kitty";
        let mut yes = FakeCaps { supported: true };
        assert!(is_kitty(
            select_renderer(RendererKind::Auto, &mut yes, 7).as_ref()
        ));
        let mut no = FakeCaps { supported: false };
        assert!(!is_kitty(
            select_renderer(RendererKind::Auto, &mut no, 7).as_ref()
        ));
        // Forced modes ignore the probe:
        let mut yes2 = FakeCaps { supported: true };
        assert!(!is_kitty(
            select_renderer(RendererKind::HalfBlock, &mut yes2, 7).as_ref()
        ));
    }

    /// The trait impl delegates the herd itself to the free function; the only
    /// thing it adds on top is the dev build marker, which is absent from a
    /// shipped build. So the two agree everywhere except the marker's own
    /// columns in the overlay lane.
    #[test]
    fn half_block_renderer_matches_the_free_function_outside_the_marker_columns() {
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working, AgentStatus::Blocked]);
        let mut via_trait = Terminal::new(TestBackend::new(60, 11)).unwrap();
        via_trait
            .draw(|f| HalfBlockRenderer.draw(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        let mut via_fn = Terminal::new(TestBackend::new(60, 11)).unwrap();
        via_fn
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();

        let lane = overlay_lane_y(Rect::new(0, 0, 60, 11)) as usize;
        let reserved = marker::reserved_cols() as usize;
        let trait_rows = rows_of(via_trait.backend());
        let fn_rows = rows_of(via_fn.backend());
        for (y, (a, b)) in trait_rows.iter().zip(fn_rows.iter()).enumerate() {
            // On the lane row, skip the columns the marker owns.
            let (a, b) = if y == lane {
                (&a[reserved.min(a.len())..], &b[reserved.min(b.len())..])
            } else {
                (&a[..], &b[..])
            };
            assert_eq!(a, b, "row {y} should match");
        }
    }

    #[test]
    fn draw_herd_does_not_panic_in_a_pane_shorter_than_the_band() {
        // Growing MEMBER_PX_H for the focus hat also grew the band's minimum
        // height; a pane too short to fit it must crop gracefully instead of
        // handing draw_pixels a Rect that overruns the real frame buffer.
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working, AgentStatus::Blocked]);
        let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
    }

    #[test]
    fn draw_herd_shows_feet_at_the_floor_of_the_shipped_five_row_strip() {
        // The shipped strip (config.rs's `strip_rows: 5`, place.rs's
        // `TARGET_ROWS: 5`) is shorter than the band the half-block renderer
        // needs (#37): the sheep loses its headroom, cropped to fit, but a
        // pane this short must still show the member's feet at the pane
        // floor, and the overlay lane (row 0) must never collide with the
        // band below it. (Kitty users don't pay this cost: `member_rows`
        // derives its band from the pane it's given, so `Auto` picking kitty
        // is unaffected either way.)
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working]);
        let mut terminal = Terminal::new(TestBackend::new(60, 5)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        let rows = rows_of(terminal.backend());
        let bottom = rows.last().expect("at least one row");
        assert!(
            bottom.contains('▀') || bottom.contains('▄'),
            "the bottom row must show the member's feet, not cropped-off blank space: {bottom:?}"
        );
        assert!(
            !rows[0].contains('▀') && !rows[0].contains('▄'),
            "the overlay lane (row 0) must hold no member pixels, even when the pane is short: {:?}",
            rows[0]
        );
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn draw_herd_shows_the_whole_band_uncropped_at_strip_rows() {
        // A half-block user who sets `strip_rows = render::STRIP_ROWS` (the
        // documented tradeoff for the full band) gets the entire member drawn
        // with no cropping at all: every half-block row of the band, plus its
        // own untouched overlay lane row above it.
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working]);
        let mut terminal = Terminal::new(TestBackend::new(60, STRIP_ROWS)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        let rows = rows_of(terminal.backend());
        assert_eq!(rows.len(), STRIP_ROWS as usize);
        let bottom = rows.last().expect("at least one row");
        assert!(
            bottom.contains('▀') || bottom.contains('▄'),
            "the bottom row must show the member's feet: {bottom:?}"
        );
        assert!(
            !rows[0].contains('▀') && !rows[0].contains('▄'),
            "the overlay lane (row 0) must hold no member pixels: {:?}",
            rows[0]
        );
        let band_rows = &rows[1..];
        assert!(
            band_rows.iter().any(|r| r.contains('▀') || r.contains('▄')),
            "the band (every row below the lane) must actually be used"
        );
    }

    /// Total non-transparent pixels in `HAT_ROWS` — how many hat pixels should
    /// land in the buffer when the hat is drawn with no clipping at all.
    const HAT_PIXEL_COUNT: usize = 9;

    fn count_hat_pixels(buf: &PixelBuf) -> usize {
        buf.px
            .iter()
            .filter(|p| matches!(p, Some(c) if *c == HAT_OUTLINE || *c == HAT_FILL))
            .count()
    }

    /// Scan `now_ms` in `[0, period_ms)` for the instant where `state`'s
    /// motion lifts `terminal_id` highest (most negative `offset.dy`) — the
    /// tightest case for top-clipping. Uses `motion::animate` as an oracle
    /// (same trick as `left_facing_member_is_mirrored` below) rather than
    /// reverse-engineering `identity::unit_hash`.
    fn peak_lift_ms(
        terminal_id: &str,
        status: AgentStatus,
        state: &crate::sprite::StateSpec,
        period_ms: u64,
    ) -> u64 {
        (0..period_ms)
            .step_by(37)
            .min_by(|&a, &b| {
                let dy_a = animate(terminal_id, status, state, a, None).offset.dy;
                let dy_b = animate(terminal_id, status, state, b, None).offset.dy;
                dy_a.partial_cmp(&dy_b).unwrap()
            })
            .expect("a non-empty scan range")
    }

    #[test]
    fn build_band_draws_a_hat_only_for_the_focused_member() {
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        // "wearer"'s hash-derived rest position sits safely away from either
        // edge (unlike e.g. "f", which happens to rest flush against the
        // right edge and would clip the hat there) — this test is about
        // focused-vs-unfocused, not edge clipping (see the dedicated
        // clipping tests below).
        let mut focused = Member::new(
            "wearer".into(),
            identity_for("wearer", 1),
            AgentStatus::Idle,
        );
        focused.focused = true;
        herd.members.push(focused);
        herd.members.push(Member::new(
            "unfocused".into(),
            identity_for("unfocused", 1),
            AgentStatus::Idle,
        ));
        let (buf, _order) = build_band(&herd, &species, 50, Theme::Dark, NOW_MS);
        assert_eq!(
            count_hat_pixels(&buf),
            HAT_PIXEL_COUNT,
            "exactly one hat is drawn, for the focused member"
        );
    }

    #[test]
    fn hat_is_never_clipped_at_the_top_even_when_the_sprite_has_no_headroom_row() {
        // TestBlob's working frame paints all the way up to row 0 (`MM..`),
        // the worst case for top clipping: no spare row inside the frame
        // itself. Scan for the instant of peak hop lift, which pushes the
        // sprite as high as it ever goes.
        let species = vec![parse_species(BLOB).unwrap()];
        let state = &species[0].states[&AgentStatus::Working];
        let peak_ms = peak_lift_ms("f", AgentStatus::Working, state, 20_000);
        let mut herd = Herd::new();
        let mut member = Member::new("f".into(), identity_for("f", 1), AgentStatus::Working);
        member.focused = true;
        herd.members.push(member);
        let (buf, _order) = build_band(&herd, &species, 40, Theme::Dark, peak_ms);
        assert_eq!(
            count_hat_pixels(&buf),
            HAT_PIXEL_COUNT,
            "every hat pixel survives — none fell off the top of the band"
        );
    }

    #[test]
    fn hat_is_never_clipped_on_the_real_sheep_standing_pose_mid_bounce() {
        // The acceptance criterion's tight case: the real (16x14) standing
        // pose, which has only ~1 empty row above the head, at peak bounce lift.
        let species = crate::sprite::embedded_species();
        let sheep_index = species
            .iter()
            .position(|s| s.name == "Sheep")
            .expect("Sheep is embedded");
        let state = &species[sheep_index].states[&AgentStatus::Blocked];
        let peak_ms = peak_lift_ms("f", AgentStatus::Blocked, state, 20_000);
        let mut herd = Herd::new();
        let mut member = Member::new(
            "f".into(),
            crate::identity::Identity {
                species_index: sheep_index,
                hue: 0,
            },
            AgentStatus::Blocked,
        );
        member.focused = true;
        herd.members.push(member);
        let (buf, _order) = build_band(&herd, &species, 40, Theme::Dark, peak_ms);
        assert_eq!(
            count_hat_pixels(&buf),
            HAT_PIXEL_COUNT,
            "the hat fits above the standing pose's head even mid-bounce"
        );
    }

    #[test]
    fn hat_renders_on_top_of_the_idle_dozing_lump() {
        let species = crate::sprite::embedded_species();
        let sheep_index = species.iter().position(|s| s.name == "Sheep").unwrap();
        let mut herd = Herd::new();
        let mut member = Member::new(
            "f".into(),
            crate::identity::Identity {
                species_index: sheep_index,
                hue: 0,
            },
            AgentStatus::Idle,
        );
        member.focused = true;
        herd.members.push(member);
        let (buf, _order) = build_band(&herd, &species, 40, Theme::Dark, NOW_MS);
        assert_eq!(
            count_hat_pixels(&buf),
            HAT_PIXEL_COUNT,
            "the hat renders above the dozing lump, not clipped or hidden"
        );
    }

    #[test]
    fn head_anchor_column_flips_with_facing() {
        // TestBlob's working frame is asymmetric (`MM../MMM./M##./.MM.`), so a
        // facing flip must move the head anchor's column — otherwise the hat
        // would stay put while the body mirrors underneath it.
        let species = parse_species(BLOB).unwrap();
        let fr = &species.states[&AgentStatus::Working].frames[0];
        let (_row_right, col_right) = head_anchor(fr, false);
        let (_row_left, col_left) = head_anchor(fr, true);
        assert_ne!(col_right, col_left, "facing flip must move the head anchor");
    }

    fn contains_color(rgba: &crate::raster::Rgba, target: Rgb) -> bool {
        rgba.px
            .chunks(4)
            .any(|p| p[3] == 255 && (p[0], p[1], p[2]) == (target.0, target.1, target.2))
    }

    #[test]
    fn stamp_hat_paints_both_hat_colors_onto_the_rgba_buffer() {
        // A large enough blank canvas that the hat (stamped near the middle)
        // can't clip against any edge.
        let mut rgba = crate::raster::Rgba {
            w: 40,
            h: 40,
            px: vec![0u8; 40 * 40 * 4],
        };
        stamp_hat(&mut rgba, 1, 10, 10, 10);
        assert!(
            contains_color(&rgba, HAT_OUTLINE),
            "the hat's outline color must be painted"
        );
        assert!(
            contains_color(&rgba, HAT_FILL),
            "the hat's fill color must be painted"
        );
    }

    #[test]
    fn stamp_hat_does_not_panic_when_it_would_clip_the_canvas_edge() {
        // head_row/head_col near (0, 0) push the hat's top-left off-canvas;
        // stamp_hat must clip gracefully rather than index out of bounds.
        let mut rgba = crate::raster::Rgba {
            w: 10,
            h: 10,
            px: vec![0u8; 10 * 10 * 4],
        };
        stamp_hat(&mut rgba, 1, 0, 0, 0);
    }

    #[test]
    fn stamp_hat_sinks_one_pixel_into_the_heads_own_row() {
        // Picked after visual review: the hat should sit into the head by
        // 1px (its bottom row covers the head's own topmost row) rather than
        // floating a clean row above it.
        let mut rgba = crate::raster::Rgba {
            w: 40,
            h: 40,
            px: vec![0u8; 40 * 40 * 4],
        };
        let (pad, head_row, head_col) = (10usize, 10usize, 10usize);
        stamp_hat(&mut rgba, 1, pad, head_row, head_col);
        let y = pad + head_row; // the head's own top row, in the padded canvas
        let row_has_hat_pixel = (0..rgba.w).any(|x| {
            let i = (y * rgba.w + x) * 4;
            rgba.px[i + 3] == 255
                && ((rgba.px[i], rgba.px[i + 1], rgba.px[i + 2])
                    == (HAT_OUTLINE.0, HAT_OUTLINE.1, HAT_OUTLINE.2)
                    || (rgba.px[i], rgba.px[i + 1], rgba.px[i + 2])
                        == (HAT_FILL.0, HAT_FILL.1, HAT_FILL.2))
        });
        assert!(
            row_has_hat_pixel,
            "the hat's bottom row must overlap the head's own top row"
        );
    }

    #[test]
    fn draw_hat_sinks_one_pixel_into_the_heads_own_row() {
        let mut buf = PixelBuf::new(20, 20);
        let (ox, oy, head_row, head_col) = (5, 10, 2usize, 8usize);
        draw_hat(&mut buf, ox, oy, head_row, head_col);
        let y = oy + head_row as i32; // the head's own top row
        let row_has_hat_pixel = (0..buf.w as i32).any(|x| {
            matches!(buf.px[y as usize * buf.w + x as usize], Some(c) if c == HAT_OUTLINE || c == HAT_FILL)
        });
        assert!(
            row_has_hat_pixel,
            "the hat's bottom row must overlap the head's own top row"
        );
    }

    // ---- issue #42: skipping frames that would change nothing ----------------

    /// A [`TestBackend`] whose error type is `io::Error`. `run_loop` is bounded
    /// on `io::Error: From<B::Error>` so the real `CrosstermBackend`'s errors
    /// reach the caller with their kind intact; `TestBackend`'s own
    /// `Infallible` doesn't satisfy that, so the tests wrap it rather than
    /// loosening the production bound.
    struct IoTestBackend {
        inner: TestBackend,
        /// How many times the terminal asked the backend for its size. In
        /// production every one of these is a `/dev/tty` open + ioctl + close,
        /// so issue #44 is really the claim that this stays at zero per frame.
        size_calls: Rc<Cell<usize>>,
    }

    impl IoTestBackend {
        fn new(width: u16, height: u16) -> (Self, Rc<Cell<usize>>) {
            let size_calls = Rc::new(Cell::new(0));
            let backend = Self {
                inner: TestBackend::new(width, height),
                size_calls: Rc::clone(&size_calls),
            };
            (backend, size_calls)
        }
    }

    /// A `Result` that cannot be an error, retyped. `match e {}` is total
    /// because `Infallible` has no variants.
    fn infallible<T>(r: Result<T, std::convert::Infallible>) -> io::Result<T> {
        match r {
            Ok(v) => Ok(v),
            Err(e) => match e {},
        }
    }

    impl ratatui::backend::Backend for IoTestBackend {
        type Error = io::Error;

        fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
        {
            infallible(self.inner.draw(content))
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            infallible(self.inner.hide_cursor())
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            infallible(self.inner.show_cursor())
        }
        fn get_cursor_position(&mut self) -> io::Result<ratatui::layout::Position> {
            infallible(self.inner.get_cursor_position())
        }
        fn set_cursor_position<P: Into<ratatui::layout::Position>>(
            &mut self,
            position: P,
        ) -> io::Result<()> {
            infallible(self.inner.set_cursor_position(position))
        }
        fn clear(&mut self) -> io::Result<()> {
            infallible(self.inner.clear())
        }
        fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> io::Result<()> {
            infallible(self.inner.clear_region(clear_type))
        }
        fn size(&self) -> io::Result<ratatui::layout::Size> {
            self.size_calls.set(self.size_calls.get() + 1);
            infallible(self.inner.size())
        }
        fn window_size(&mut self) -> io::Result<ratatui::backend::WindowSize> {
            infallible(self.inner.window_size())
        }
        fn flush(&mut self) -> io::Result<()> {
            infallible(self.inner.flush())
        }
    }

    /// Counts `draw` calls and records the area each one drew at, so a test can
    /// assert a frame was *not* painted. Its `frame_signature` delegates to the
    /// shipped half-block one, so the loop under test is gated on the real
    /// thing rather than a test-only stand-in.
    #[derive(Default)]
    struct CountingRenderer {
        draws: usize,
        areas: Vec<Rect>,
    }

    impl MemberRenderer for CountingRenderer {
        fn draw(
            &mut self,
            frame: &mut Frame,
            _herd: &Herd,
            _species: &[Species],
            _theme: Theme,
            _now_ms: u64,
            _hover_label: Option<&str>,
        ) {
            self.draws += 1;
            self.areas.push(frame.area());
        }
        fn frame_signature(
            &self,
            herd: &Herd,
            species: &[Species],
            theme: Theme,
            area: Rect,
            now_ms: u64,
            hover_label: Option<&str>,
        ) -> u64 {
            band_signature(herd, species, theme, area, now_ms, hover_label)
        }
        fn member_at_column(
            &self,
            _herd: &Herd,
            _species: &[Species],
            _strip_w: usize,
            _col: u16,
            _now_ms: u64,
        ) -> Option<usize> {
            None
        }
        fn backend_name(&self) -> &'static str {
            "counting"
        }
    }

    /// Replays a fixed script of loop ticks: `None` is a 12 fps timeout with no
    /// input, `Some(e)` an event. Once the script runs out it presses `q`, so a
    /// test can never hang in `run_loop`'s infinite loop.
    struct ScriptedEvents {
        script: std::vec::IntoIter<Option<Event>>,
    }

    impl ScriptedEvents {
        fn new(script: Vec<Option<Event>>) -> Self {
            Self {
                script: script.into_iter(),
            }
        }
    }

    impl EventSource for ScriptedEvents {
        fn poll_event(&mut self, _timeout: Duration) -> io::Result<Option<Event>> {
            Ok(match self.script.next() {
                Some(ev) => ev,
                None => Some(Event::Key(KeyCode::Char('q').into())),
            })
        }
    }

    struct SilentPlayer;
    impl crate::sound::SoundPlayer for SilentPlayer {
        fn play(&self, _path: &std::path::Path) -> io::Result<()> {
            Ok(())
        }
    }

    /// Drive the real `run_loop` over `script`, starting from a herd of
    /// `states`, and hand back the renderer so the caller can inspect what it
    /// was actually asked to draw. `reduced_motion` pins `now_ms` to 0, which
    /// makes every frame byte-identical: exactly the case issue #42 measured
    /// as 1197 zero-change frames out of 1199.
    fn drive_run_loop(
        backend: IoTestBackend,
        width: u16,
        height: u16,
        states: &[AgentStatus],
        script: Vec<Option<Event>>,
    ) -> CountingRenderer {
        let species = vec![parse_species(BLOB).unwrap()];
        let agents: Vec<_> = states
            .iter()
            .enumerate()
            .map(|(i, s)| agent(&format!("t{i}"), *s))
            .collect();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(agents).expect("the receiver is still alive");
        drop(tx); // no further snapshots: the herd never changes again

        let cli = LiveHerdr::with_runner(
            "herdr",
            Recorder {
                args: Rc::new(RefCell::new(Vec::new())),
            },
        );
        // A fixed viewport, exactly as `run` builds it, so the loop under test
        // is the one that ships (a fullscreen viewport would autoresize and
        // hide the `Event::Resize` handling).
        let viewport = Viewport::Fixed(Rect::new(0, 0, width, height));
        let mut terminal = Terminal::with_options(backend, TerminalOptions { viewport }).unwrap();
        let mut renderer = CountingRenderer::default();
        let mut events = ScriptedEvents::new(script);
        run_loop(
            &mut terminal,
            rx,
            &species,
            Theme::Dark,
            &cli,
            true, // reduced motion: `now_ms` is pinned, so nothing can move
            &mut renderer,
            &crate::config::SoundConfig::default(),
            &SilentPlayer,
            &mut events,
        )
        .expect("the scripted loop quits cleanly");
        renderer
    }

    #[test]
    fn an_unchanging_herd_is_drawn_once_and_then_skipped() {
        use AgentStatus::*;
        // 40 idle ticks after the herd arrives. Only the first frame has
        // anything new to say; the other 40 must not reach the renderer at all.
        let (backend, size_calls) = IoTestBackend::new(90, 11);
        let renderer = drive_run_loop(
            backend,
            90,
            11,
            &[Idle, Done, Working, Blocked],
            vec![None; 40],
        );
        assert_eq!(
            renderer.draws, 1,
            "an unchanged strip must be painted once, not once per tick"
        );
        // Issue #44: a fixed viewport never autoresizes, and the loop no longer
        // asks for the size itself, so 41 ticks must cost zero `/dev/tty`
        // opens.
        assert_eq!(
            size_calls.get(),
            0,
            "the render loop must not query the terminal size per frame"
        );
    }

    #[test]
    fn a_resize_repaints_even_though_the_herd_did_not_change() {
        use AgentStatus::*;
        // The regression that would otherwise ship silently: with an unchanged
        // herd every frame after the first is skipped, so the resize event is
        // the *only* thing that can force the strip to reflow.
        let (backend, _) = IoTestBackend::new(90, 11);
        let renderer = drive_run_loop(
            backend,
            90,
            11,
            &[Idle, Done],
            vec![None, None, Some(Event::Resize(60, 11)), None, None],
        );
        assert_eq!(
            renderer.areas,
            vec![Rect::new(0, 0, 90, 11), Rect::new(0, 0, 60, 11)],
            "the strip is painted once at the old size and again at the new one"
        );
    }

    #[test]
    fn a_resize_to_the_same_geometry_does_not_repaint() {
        use AgentStatus::*;
        // Terminals emit resize events for geometry that did not change.
        // Acting on one would clear the viewport, and the frame that would
        // have repainted it is the one the signature says to skip.
        let (backend, _) = IoTestBackend::new(90, 11);
        let renderer = drive_run_loop(
            backend,
            90,
            11,
            &[Idle, Done],
            vec![None, Some(Event::Resize(90, 11)), None],
        );
        assert_eq!(renderer.draws, 1, "a no-op resize must stay a no-op");
    }

    #[test]
    fn the_frame_signature_changes_when_the_pane_is_resized() {
        // The one trap in the whole change: leave `area` out of the signature
        // and a resized pane keeps showing the old layout forever.
        use AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[Idle, Working]);
        let sig = |w: u16, h: u16| {
            band_signature(
                &herd,
                &species,
                Theme::Dark,
                Rect::new(0, 0, w, h),
                NOW_MS,
                None,
            )
        };
        assert_eq!(sig(90, 11), sig(90, 11), "same area, same frame");
        assert_ne!(sig(90, 11), sig(60, 11), "a narrower pane must repaint");
        assert_ne!(sig(90, 11), sig(90, 8), "a shorter pane must repaint");
    }

    #[test]
    fn the_frame_signature_changes_when_the_hover_caption_changes() {
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Idle]);
        let area = Rect::new(0, 0, 90, 11);
        let sig =
            |label: Option<&str>| band_signature(&herd, &species, Theme::Dark, area, NOW_MS, label);
        assert_ne!(sig(None), sig(Some("sheep")), "showing a name repaints");
        assert_ne!(
            sig(Some("sheep")),
            sig(Some("goat")),
            "a different name repaints"
        );
    }

    /// Render the strip at `now_ms` and return the finished cells as text:
    /// the ground truth a signature must never disagree with.
    fn strip_at(herd: &Herd, species: &[Species], area: Rect, now_ms: u64) -> String {
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        let mut renderer = HalfBlockRenderer;
        terminal
            .draw(|f| renderer.draw(f, herd, species, Theme::Dark, now_ms, None))
            .unwrap();
        format!("{}", terminal.backend())
    }

    /// Every distinct signature seen over `sweep`, mapped to the strip that was
    /// rendered the first time it appeared.
    fn sweep_signatures(
        herd: &Herd,
        species: &[Species],
        area: Rect,
        sweep: impl Iterator<Item = u64>,
    ) -> (usize, std::collections::HashMap<u64, String>) {
        let mut seen: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
        let mut frames = 0;
        for now_ms in sweep {
            frames += 1;
            let sig = band_signature(herd, species, Theme::Dark, area, now_ms, None);
            let strip = strip_at(herd, species, area, now_ms);
            match seen.get(&sig) {
                Some(first) => assert_eq!(
                    first, &strip,
                    "two frames hashed the same at {now_ms} ms but do not look the same; \
                     skipping the second would drop a visible change"
                ),
                None => {
                    seen.insert(sig, strip);
                }
            }
        }
        (frames, seen)
    }

    #[test]
    fn a_repeated_frame_signature_always_means_an_identical_strip() {
        // The safety property the skip rests on: same signature => same pixels.
        // Swept over 60 s of animation at 12 fps for a mixed herd, so walking
        // members, hopping members and dozing ones all get their turn.
        use AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[Working, Working, Idle, Done, Blocked, Unknown]);
        let area = Rect::new(0, 0, 90, 11);
        let (frames, seen) = sweep_signatures(&herd, &species, area, (0..60_000).step_by(83));
        assert!(
            seen.len() < frames,
            "the sweep never repeated a signature, so it proved nothing"
        );
    }

    #[test]
    fn an_idle_herd_holds_one_signature_across_a_minute_of_animation() {
        // The win, stated as a test: idle members breathe continuously, but the
        // breath rounds away before it reaches a pixel, so 60 s of frames
        // collapse to a single repaint.
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Idle; 4]);
        let area = Rect::new(0, 0, 200, 11);
        let (frames, seen) = sweep_signatures(&herd, &species, area, (0..60_000).step_by(83));
        assert_eq!(frames, 723, "60 s at ~12 fps");
        assert_eq!(
            seen.len(),
            1,
            "an all-idle strip must collapse to one drawn frame, got {}",
            seen.len()
        );
    }
}
