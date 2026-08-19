//! Kitty graphics protocol backend for [`MemberRenderer`]: transmits each
//! distinct sprite frame once and caches the image id, places/re-places it at
//! the member's cell every frame, and deletes placements for members that departed
//! or fell out of the visible set. Escapes are written to an injected
//! `io::Write` sink (real stdout in production, a `Vec<u8>`-backed sink in
//! tests) so the encoding is unit-testable without a real terminal.
//!
//! This renderer does NOT own the terminal. Every strip pane is a separate
//! process forwarding escapes to one outer terminal, so image ids, deletes and
//! image memory are all terminal-global and shared. Three rules follow, and the
//! tests at the bottom pin each of them by driving *two* renderers at once:
//! ids come from this pane's own block of the id space (see [`crate::kitty_ids`]),
//! deletes always name an id this pane owns (never `d=A`), and images this pane
//! stopped placing are freed rather than left resident forever.

use std::collections::HashMap;
use std::io::{self, Write};

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::agent::AgentStatus;
use crate::anim::{Overlay, OverlayColor};
use crate::herd::{Herd, visible_and_hidden};
use crate::icon::{IconKind, icon_size, rasterize_icon};
use crate::kitty::{Crop, delete_image, delete_placement, place_cropped, transmit_rgba};
use crate::kitty_ids::{ImageIds, PlacementIds};
use crate::member::priority;
use crate::motion::animate;
use crate::palette::{StateStyle, Theme, role_color};
use crate::raster::{pad_frame, rasterize};
use crate::render::{HAT_H, MemberRenderer, head_anchor, stamp_hat};
use crate::sprite::{Frame as SpriteFrame, Species};

/// Sprite-pixel margin padded around a transmitted member image, so a motion
/// offset can be animated by panning a crop window instead of retransmitting.
/// 2px comfortably covers every motion's max amplitude (breathe <=0.5, hop/
/// bounce <=1.0, sway <=1.0). The walking hop lifts up to 2px, still covered.
const MOTION_PAD: usize = 2;

/// Sprite-pixels of transparent headroom the *displayed* crop window keeps
/// ABOVE the sprite, so the baked-in focus hat (`HAT_H`) and the walking hop
/// (`<=2px`) never clip the head. Reserved for every member (focused or not) so
/// all sheep render at one consistent size, and folded into the same cell
/// footprint — so the visible sheep is simply drawn a little smaller to make
/// the room, rather than the strip growing taller.
const TOP_HEADROOM: usize = HAT_H + 2;

/// Icon-pixel margin padded around a transmitted overlay icon image, for the
/// same crop-panning trick. `ICON_PAD - ICON_MARGIN` is the pan range each
/// side of the displayed (bitmap + margin) crop actually gets, so it must
/// clear `icon_wave_offset`'s max amplitude (dy <= 1.0) with room to spare —
/// otherwise the wave's rise pans the crop straight into the bitmap itself,
/// clipping the glyph's top edge during motion.
const ICON_PAD: usize = 6;

/// Extra transparent framing (icon-pixels, on top of the bitmap's own size)
/// included in the *displayed* crop, so the glyph doesn't fill its on-screen
/// cell edge-to-edge — the bitmaps themselves are full-bleed (e.g. `Sleep`'s
/// top/bottom rows are solid ink), so without this margin the icon reads as
/// a big, blocky mark rather than a small floating one.
const ICON_MARGIN: usize = 2;

/// Stacking index floor for overlay icons: always above every member body, which
/// draw at `z = 0..order.len()`.
const Z_ICON_BASE: i32 = 1000;

/// The source rectangle (in raster pixels) that shows an unpadded `w`x`h`
/// (at `scale` px/unit) region shifted by `(dx, dy)` units within a canvas
/// padded by `pad` units on every side — panning a same-size "camera" over a
/// larger, static image to fake motion without retransmitting it. Clamped so
/// a same-size larger offset pins to the padding edge instead of overflowing.
fn crop_rect(pad: usize, scale: usize, w: usize, h: usize, dx: f32, dy: f32) -> Crop {
    let scale_f = scale as f32;
    let max_x = (2 * pad * scale) as i32;
    let max_y = (2 * pad * scale) as i32;
    let x = (((pad as f32 - dx) * scale_f).round() as i32).clamp(0, max_x);
    let y = (((pad as f32 - dy) * scale_f).round() as i32).clamp(0, max_y);
    Crop {
        x: x as u32,
        y: y as u32,
        w: (w * scale) as u32,
        h: (h * scale) as u32,
    }
}

/// The displayed crop for a member: `sprite_w` wide but `sprite_h + top_headroom`
/// tall — the extra rows sit ABOVE the sprite so the baked-in hat and the
/// walking hop never clip the head (unlike a plain `sprite_h`-tall window,
/// which pans straight off the top of the head on the upbeat). Bottom-anchored
/// on the feet and panned by the motion offset, exactly like [`crop_rect`],
/// then placed into the same cell footprint so the sheep just renders smaller.
/// `body_pad` must be `>= top_headroom` (the rest offset above the sprite) and
/// `>=` the max hop (the downward pan room) — `MOTION_PAD + HAT_H` satisfies both.
fn member_crop(
    body_pad: usize,
    top_headroom: usize,
    scale: usize,
    sprite_w: usize,
    sprite_h: usize,
    dx: f32,
    dy: f32,
) -> Crop {
    let s = scale as f32;
    let win_h = sprite_h + top_headroom;
    // Horizontal pan: identical to `crop_rect` — centered on the sprite, swayed
    // by dx, clamped inside the horizontal padding.
    let max_x = (2 * body_pad * scale) as f32;
    let x = (((body_pad as f32) - dx) * s).round().clamp(0.0, max_x) as u32;
    // Vertical: at rest the window starts `top_headroom` rows above the sprite
    // (revealing the hat); a hop (dy<0) shifts it down so the feet lift off the
    // floor. Clamped so it never reads past the padded canvas's bottom.
    let y_rest = (body_pad - top_headroom) as f32;
    let max_y = ((2 * body_pad - top_headroom) * scale) as f32;
    let y = ((y_rest - dy) * s).round().clamp(0.0, max_y) as u32;
    Crop {
        x,
        y,
        w: (sprite_w * scale) as u32,
        h: (win_h * scale) as u32,
    }
}

/// Rows a member image occupies, derived from the pane height and capped small
/// so the members always read as a slim strip, even in a tall pane. Reserves 1
/// row off the top for the shared overlay lane (see [`overlay_lane_row`]) —
/// the caption and every member's icon/hat all live there, column-separated,
/// exactly like the half-block renderer's single `lane_y` row, rather than
/// each claiming a row of its own.
fn member_rows(pane_h: u16) -> u16 {
    pane_h.saturating_sub(1).clamp(2, 4)
}

/// The 1-based cursor row a `rows`-tall member image must start at so its last
/// occupied row is exactly `pane_h` — the pane's own last row — with no gap
/// beneath it, mirroring the half-block path's bottom-anchor.
fn member_row(pane_h: i32, rows: u16) -> i32 {
    (pane_h - rows as i32 + 1).clamp(1, pane_h.max(1))
}

/// The 1-indexed strip row reserved exclusively for the hover caption (the
/// member's *name*), drawn via the ratatui frame in [`KittyRenderer::draw`]. It
/// sits one row above the member band and carries **no kitty image ever** — the
/// per-member Zz/!/? icons now float inside the band's own headroom (above the
/// shrunk sheep's head), and the focus hat is baked into the member image. A
/// dedicated, image-free row is what stops a member's icon or hopping head from
/// painting over the name (which made it flicker on then vanish) and
/// guarantees it can never overlap a sheep.
fn overlay_lane_row(pane_h: u16) -> u16 {
    let rows = member_rows(pane_h);
    member_row(pane_h as i32, rows).saturating_sub(1).max(1) as u16
}

/// Columns for a `frame_w` x `frame_h` sprite shown at `rows` rows, preserving
/// the sprite's aspect under an assumed ~1:2.1 cell width:height ratio. herdr
/// hides the real cell size, so we approximate it; the worst case is a slight
/// horizontal stretch, imperceptible for pixel art. Placing with explicit
/// `c=`/`r=` (rather than native pixels) makes the on-screen footprint exact,
/// which is what lets hover hit-testing line up with the visible sprite.
fn member_cols(rows: u16, frame_w: usize, frame_h: usize) -> u16 {
    ((rows as f32) * (frame_w as f32 / frame_h.max(1) as f32) * 2.1)
        .round()
        .max(1.0) as u16
}

/// The half-open range of columns `[lo, hi)` that carry opaque pixels in
/// `frame` as it is drawn (mirrored when `flip`), or `None` if the frame is
/// fully transparent. Transparency is independent of hue/theme/style, so a
/// fixed palette lookup suffices. Used to trim hover hit-testing to the visible
/// sprite rather than its full, transparently-padded frame width.
fn opaque_col_span(frame: &SpriteFrame, flip: bool) -> Option<(usize, usize)> {
    let (mut lo, mut hi, mut any) = (frame.w, 0usize, false);
    for y in 0..frame.h {
        for dx in 0..frame.w {
            let sx = if flip { frame.w - 1 - dx } else { dx };
            if role_color(
                frame.cells[y * frame.w + sx],
                0,
                Theme::Dark,
                StateStyle::none(),
            )
            .is_some()
            {
                any = true;
                lo = lo.min(dx);
                hi = hi.max(dx);
            }
        }
    }
    any.then_some((lo, hi + 1))
}

/// Cache key for a transmitted image: species/status/frame/flip/hue/focused
/// fully determine the rasterized pixels (focused bakes the hat directly
/// into the image — see `render::stamp_hat`), so two members sharing all six
/// reuse one image id instead of retransmitting.
type ImgKey = (usize, AgentStatus, usize, bool, u16, bool);

/// A transmitted image: the id it lives under in the terminal, and the frame
/// this renderer last placed it on. The frame stamp drives eviction — without
/// it nothing ever frees image data, so a long-lived pane accumulates every
/// hue/status/frame combination it has ever drawn (issue #30).
#[derive(Debug, Clone, Copy)]
struct Cached {
    id: u32,
    last_used: u64,
}

/// Frames an image is kept after the last time it was placed. The render loop
/// ticks at ~12 fps, so ~60s of not being drawn. Counted in frames rather than
/// `now_ms` because reduced-motion mode pins `now_ms` to 0 for the life of the
/// process, which would make a wall-clock TTL never fire.
const IMAGE_TTL_FRAMES: u64 = 720;

/// Backstop cap on cached images (members and icons counted separately), for
/// bursts that churn faster than the TTL — a herd cycling through many distinct
/// agent ids, each contributing its own hue. Comfortably above the live working
/// set (visible members x animation frames), and images placed on the current
/// frame are never evicted, so this can only reclaim images that are off screen.
const MAX_CACHED_IMAGES: usize = 256;

/// Drop every entry of `cache` that has not been placed for
/// [`IMAGE_TTL_FRAMES`], then the oldest entries above [`MAX_CACHED_IMAGES`],
/// returning the image ids whose terminal-side data must now be freed. Entries
/// placed on `frame` itself are never dropped — they are on screen.
fn evict_from<K: Eq + std::hash::Hash + Clone>(
    cache: &mut HashMap<K, Cached>,
    frame: u64,
) -> Vec<u32> {
    let mut freed = Vec::new();
    cache.retain(|_, c| {
        let stale = frame.saturating_sub(c.last_used) > IMAGE_TTL_FRAMES;
        if stale {
            freed.push(c.id);
        }
        !stale
    });
    if cache.len() > MAX_CACHED_IMAGES {
        let mut by_age: Vec<(K, u64)> = cache
            .iter()
            .filter(|(_, c)| c.last_used < frame)
            .map(|(k, c)| (k.clone(), c.last_used))
            .collect();
        by_age.sort_by_key(|(_, last_used)| *last_used);
        let excess = cache.len() - MAX_CACHED_IMAGES;
        for (key, _) in by_age.into_iter().take(excess) {
            if let Some(c) = cache.remove(&key) {
                freed.push(c.id);
            }
        }
    }
    freed
}

/// Draws the herd via the kitty graphics protocol instead of ratatui cells.
/// Images are transmitted once per distinct `ImgKey` and cached; each visible
/// member gets a placement that is re-created every frame (draw-then-delete-old,
/// to avoid a flicker where the member briefly vanishes).
pub struct KittyRenderer {
    scale: usize,
    out: Box<dyn Write + Send>,
    cache: HashMap<ImgKey, Cached>,
    /// `terminal_id -> (image_id, placement_id)` of that member's current
    /// placement. The image id is tracked alongside the placement id (not
    /// just the placement id, per the plan's original shape) because a
    /// redraw can move the member onto a *different* cached image — a new
    /// status, frame, or facing direction — and deleting the previous
    /// on-screen placement requires the image id it was placed under, which
    /// a `HashMap<String, u32>` of placement ids alone cannot recover.
    placements: HashMap<String, (u32, u32)>,
    /// Transmitted overlay icon images, cached by (icon kind, is-dark-theme,
    /// resolved overlay color) since an icon's pixels don't depend on
    /// species/hue/facing, but `done` and `blocked` share `IconKind::Alert`
    /// (both use `!`) and must render as distinct images (accent vs. red).
    icon_cache: HashMap<(IconKind, bool, OverlayColor), Cached>,
    /// `terminal_id -> (image_id, placement_id)` of that member's current icon
    /// placement, if its state has an overlay. Tracked separately from
    /// `placements` because a member can lose its overlay (e.g. idle -> working)
    /// while staying visible, which must delete the icon but keep the member.
    icon_placements: HashMap<String, (u32, u32)>,
    /// This pane's own block of the terminal-global image-id space. Counting
    /// from 1 in every process would have panes overwrite each other's images
    /// (issue #29).
    image_ids: ImageIds,
    /// Placement ids come from their own counter: they are allocated per member
    /// per frame, and sharing the image counter (as this once did) would burn
    /// through the id block at frame rate.
    placement_ids: PlacementIds,
    /// Frames drawn, the clock for image eviction. See [`IMAGE_TTL_FRAMES`].
    frame: u64,
    /// The pane area the last frame was drawn against. When it changes (a
    /// resize), the terminal may have dropped our transmitted images, so we
    /// invalidate the cache and clear the screen state to force a fresh
    /// transmit — otherwise `place` would reference images that no longer
    /// exist and the strip would go permanently blank after a resize.
    last_area: Option<Rect>,
}

impl KittyRenderer {
    /// Build a renderer that writes to `out` (stdout in production). `scale` is
    /// the pixels-per-sprite-pixel resolution the frames are rasterized at; the
    /// on-screen size is set separately by the explicit cell footprint
    /// (`member_rows`/`member_cols`) at placement time.
    pub fn new(scale: usize, out: Box<dyn Write + Send>) -> Self {
        Self::with_image_ids(scale, out, ImageIds::for_process())
    }

    /// Build a renderer over an explicit id block. Production goes through
    /// [`KittyRenderer::new`], which derives the block from the process; the
    /// block is injected here so a test can drive two renderers — standing in
    /// for two panes — with disjoint id spaces inside one process.
    pub fn with_image_ids(scale: usize, out: Box<dyn Write + Send>, image_ids: ImageIds) -> Self {
        Self {
            scale,
            out,
            cache: HashMap::new(),
            placements: HashMap::new(),
            icon_cache: HashMap::new(),
            icon_placements: HashMap::new(),
            image_ids,
            placement_ids: PlacementIds::new(),
            frame: 0,
            last_area: None,
        }
    }

    /// Free every image this pane transmitted, and with them (uppercase `d=I`)
    /// their placements. Scoped to ids from this pane's own block: the
    /// protocol's `a=d,d=A` would take every *other* pane's images down too,
    /// and their caches would keep placing the dead ids forever (issue #28).
    fn free_all_images(&mut self) -> io::Result<()> {
        let ids: Vec<u32> = self
            .cache
            .values()
            .chain(self.icon_cache.values())
            .map(|c| c.id)
            .collect();
        for id in ids {
            self.out.write_all(delete_image(id).as_bytes())?;
        }
        self.cache.clear();
        self.icon_cache.clear();
        // The placements died with their images; forget them so no later frame
        // tries to delete a placement of an image that is already gone.
        self.placements.clear();
        self.icon_placements.clear();
        Ok(())
    }

    /// Free images this pane has not placed for [`IMAGE_TTL_FRAMES`], plus the
    /// oldest ones above [`MAX_CACHED_IMAGES`]. Nothing placed on the current
    /// frame is touched, so this can only reclaim images that are off screen.
    fn evict_stale_images(&mut self) -> io::Result<()> {
        let frame = self.frame;
        let mut freed = Vec::new();
        freed.extend(evict_from(&mut self.cache, frame));
        freed.extend(evict_from(&mut self.icon_cache, frame));
        for id in freed {
            self.out.write_all(delete_image(id).as_bytes())?;
        }
        Ok(())
    }

    /// Write all escapes for the current frame's visible members to `self.out`,
    /// replicating `render::draw_herd`'s visible-set + z-order selection
    /// exactly so the kitty and half-block backends agree on what's shown.
    fn render_members(
        &mut self,
        herd: &Herd,
        species: &[Species],
        area: Rect,
        theme: Theme,
        now_ms: u64,
    ) -> io::Result<()> {
        self.frame += 1;

        // On a geometry change (resize), the terminal may have dropped our
        // transmitted images. Purge everything and re-transmit fresh this
        // frame, or `place` would reference gone images and leave the strip
        // blank. The per-member positions below already reflow to the new area.
        // The purge frees only *this* pane's ids — a resize here must not
        // disturb any other pane's images (issue #28).
        if self.last_area != Some(area) {
            self.free_all_images()?;
            self.last_area = Some(area);
        }

        let strip_w = area.width as usize;
        let member_w = species.first().map(|s| s.size().0).unwrap_or(12);
        let max_x = (strip_w as f32 - member_w as f32).max(0.0);
        let capacity = (strip_w / (member_w * 3 / 4).max(1)).max(1);
        let (visible, _hidden) = visible_and_hidden(&herd.members, capacity);

        let mut order = visible.clone();
        order.sort_by_key(|&i| priority(herd.members[i].status));

        for (zi, &i) in order.iter().enumerate() {
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
            let animated = animate(
                &member.terminal_id,
                member.status,
                state,
                now_ms,
                member.anchor,
            );
            let fr = &state.frames[animated.frame_index];
            let key: ImgKey = (
                member.identity.species_index,
                member.status,
                animated.frame_index,
                animated.facing_left,
                member.identity.hue,
                member.focused,
            );
            // Every member's image reserves hat + hop headroom above the sprite
            // (the hat is baked directly into this same image, not a separate
            // placement), so all sheep rasterize at one size whether or not
            // they're focused — and the headroom-inclusive crop below can keep
            // TOP_HEADROOM rows above the head without ever clipping it.
            let body_pad = MOTION_PAD + HAT_H;
            let image_id = match self.cache.get_mut(&key) {
                Some(cached) => {
                    // Touch it: an image placed this frame is not evictable.
                    cached.last_used = self.frame;
                    cached.id
                }
                None => {
                    let style = StateStyle {
                        dim: state.dim,
                        ghost: state.ghost,
                    };
                    // Transmit a padded canvas (once per key), so this state's
                    // motion can be animated below by panning a crop window
                    // over it instead of retransmitting every frame.
                    let padded = pad_frame(fr, body_pad);
                    let mut rgba = rasterize(
                        &padded,
                        member.identity.hue,
                        theme,
                        style,
                        self.scale,
                        animated.facing_left,
                    );
                    if member.focused {
                        // Stamped in pixel space, using the exact same head
                        // anchor the half-block renderer uses — pose-agnostic,
                        // so it lands correctly on every pose (including the
                        // idle "dozing" lump, whose top rows are transparent
                        // padding — see sprites/sheep.sprite).
                        let (head_row, head_col) = head_anchor(fr, animated.facing_left);
                        stamp_hat(&mut rgba, self.scale, body_pad, head_row, head_col);
                    }
                    let id = self.image_ids.alloc();
                    self.out
                        .write_all(transmit_rgba(id, rgba.w, rgba.h, &rgba.px).as_bytes())?;
                    self.cache.insert(
                        key,
                        Cached {
                            id,
                            last_used: self.frame,
                        },
                    );
                    id
                }
            };

            // Size the member to an explicit cell footprint (herdr hides the real
            // cell size), then bottom-anchor it so its feet rest on the true
            // pane floor, matching the half-block path. Recomputed every
            // frame, so a resize reflows the members. Cursor coordinates are
            // 1-based and clamped into the pane so an edge member is never
            // placed off-screen.
            let rows = member_rows(area.height);
            // Size the on-screen footprint to the *headroom-inclusive* window
            // (fr.h + TOP_HEADROOM), so the sheep shrinks a little to leave room
            // for the hat/hop above it — rather than the strip growing taller.
            let cols = member_cols(rows, fr.w, fr.h + TOP_HEADROOM);
            let pane_h = area.height as i32;
            let row = member_row(pane_h, rows);
            let col = ((animated.x_fraction * max_x).round() as i32 + 1)
                .clamp(1, area.width.max(1) as i32);
            self.out
                .write_all(format!("\x1b[{row};{col}H").as_bytes())?;

            let pid = self.placement_ids.alloc();
            // Pan the crop window by this state's motion offset (breathe/hop/
            // bounce/sway) — the same offset the half-block path bakes
            // straight into its pixel buffer — so the body (and its baked-in
            // hat, if any) actually animates instead of sitting dead still.
            let crop = member_crop(
                body_pad,
                TOP_HEADROOM,
                self.scale,
                fr.w,
                fr.h,
                animated.offset.dx,
                animated.offset.dy,
            );
            // z = draw-order index: later-drawn (higher priority) stacks on top,
            // so kitty's visual stacking matches the hit-test's last-wins order.
            self.out
                .write_all(place_cropped(image_id, pid, crop, cols, rows, zi as i32).as_bytes())?;

            if let Some((old_img, old_pid)) = self
                .placements
                .insert(member.terminal_id.clone(), (image_id, pid))
            {
                // Draw-then-delete: the new placement is already on screen
                // before the old one disappears, so there is no blank frame.
                self.out
                    .write_all(delete_placement(old_img, old_pid).as_bytes())?;
            }

            // Overlay icon: a small pixel-art Zz/!/? floating just above the
            // member, on its own wave motion (`member.icon_phase`), independent of
            // the body's own state. No overlay this state -> drop any
            // lingering icon placement from a previous status.
            let glyph = match &state.overlay.kind {
                Overlay::Bubble(g) | Overlay::Badge(g) => Some(g.as_str()),
                Overlay::None => None,
            };
            match glyph.and_then(IconKind::from_glyph) {
                Some(kind) => {
                    let icon_key = (kind, theme == Theme::Dark, state.overlay.color);
                    let icon_image_id = match self.icon_cache.get_mut(&icon_key) {
                        Some(cached) => {
                            cached.last_used = self.frame;
                            cached.id
                        }
                        None => {
                            let rgba = rasterize_icon(
                                kind,
                                theme,
                                state.overlay.color,
                                self.scale,
                                ICON_PAD,
                            );
                            let id = self.image_ids.alloc();
                            self.out.write_all(
                                transmit_rgba(id, rgba.w, rgba.h, &rgba.px).as_bytes(),
                            )?;
                            self.icon_cache.insert(
                                icon_key,
                                Cached {
                                    id,
                                    last_used: self.frame,
                                },
                            );
                            id
                        }
                    };
                    let (iw, ih) = icon_size(kind);
                    // Crop a slightly larger window than the bitmap itself, so
                    // the on-screen cell keeps transparent framing around the
                    // glyph instead of the (full-bleed) bitmap filling it edge
                    // to edge — see `ICON_MARGIN`. `crop_rect` centers its
                    // crop window `pad` units in from the transmitted
                    // canvas's edge; since this crop window is itself
                    // `ICON_MARGIN` units larger than the bitmap it's
                    // centered on, the pad passed here must shrink by that
                    // same margin, or the "rest" position ends up flush
                    // against the bitmap's own top-left edge (no framing
                    // above/left at all) instead of framing it symmetrically —
                    // which is exactly what previously let the wave's rise
                    // crop straight into the glyph's top row.
                    let icon_crop = crop_rect(
                        ICON_PAD - ICON_MARGIN,
                        self.scale,
                        iw + ICON_MARGIN * 2,
                        ih + ICON_MARGIN * 2,
                        animated.icon_offset.dx,
                        animated.icon_offset.dy,
                    );
                    let icon_rows: u16 = 1;
                    let icon_cols = member_cols(icon_rows, iw, ih);
                    // Float the icon in the member band's own headroom (its top
                    // cell), above the shrunk sheep's head — NOT the top lane,
                    // which is now the dedicated, kitty-image-free name row, so
                    // the hover caption there can never be painted over by an
                    // icon (the old shared-lane collision that made the name
                    // flicker on and vanish).
                    let icon_row = row;
                    let icon_col_max = (area.width as i32 - icon_cols as i32 + 1).max(1);
                    let icon_col =
                        (col + (cols as i32) / 2 - (icon_cols as i32) / 2).clamp(1, icon_col_max);
                    self.out
                        .write_all(format!("\x1b[{icon_row};{icon_col}H").as_bytes())?;
                    let icon_pid = self.placement_ids.alloc();
                    self.out.write_all(
                        place_cropped(
                            icon_image_id,
                            icon_pid,
                            icon_crop,
                            icon_cols,
                            icon_rows,
                            Z_ICON_BASE + zi as i32,
                        )
                        .as_bytes(),
                    )?;
                    if let Some((old_img, old_pid)) = self
                        .icon_placements
                        .insert(member.terminal_id.clone(), (icon_image_id, icon_pid))
                    {
                        self.out
                            .write_all(delete_placement(old_img, old_pid).as_bytes())?;
                    }
                }
                None => {
                    if let Some((old_img, old_pid)) =
                        self.icon_placements.remove(&member.terminal_id)
                    {
                        // This status has no overlay (e.g. working) — drop any
                        // icon left over from a previous status (e.g. idle).
                        self.out
                            .write_all(delete_placement(old_img, old_pid).as_bytes())?;
                    }
                }
            }
        }

        // Any tracked member not in the current visible set (departed, or pushed
        // out by overflow) no longer belongs on screen: drop its placement so
        // it doesn't linger as a ghost image.
        let visible_ids: std::collections::HashSet<&str> = visible
            .iter()
            .map(|&i| herd.members[i].terminal_id.as_str())
            .collect();
        let departed: Vec<String> = self
            .placements
            .keys()
            .filter(|tid| !visible_ids.contains(tid.as_str()))
            .cloned()
            .collect();
        for tid in departed {
            if let Some((img, pid)) = self.placements.remove(&tid) {
                self.out.write_all(delete_placement(img, pid).as_bytes())?;
            }
        }
        let departed_icons: Vec<String> = self
            .icon_placements
            .keys()
            .filter(|tid| !visible_ids.contains(tid.as_str()))
            .cloned()
            .collect();
        for tid in departed_icons {
            if let Some((img, pid)) = self.icon_placements.remove(&tid) {
                self.out.write_all(delete_placement(img, pid).as_bytes())?;
            }
        }

        // Deleting the placements above only takes images off screen; their
        // pixel data stays resident in the terminal until it is explicitly
        // freed. Hand back what this pane has stopped drawing (issue #30).
        self.evict_stale_images()?;

        Ok(())
    }

    /// Draw the hover caption as direct terminal escapes on the dedicated name
    /// row — bypassing ratatui, whose text the
    /// per-frame kitty re-placement clobbers and then never redraws (see
    /// [`KittyRenderer::draw`]). The row carries no member image, so it's cleared
    /// and rewritten every frame with no stale trail. Row/column are 1-indexed
    /// within this pane's own terminal.
    fn draw_overlay_text(&mut self, area: Rect, hover_label: Option<&str>) -> io::Result<()> {
        if area.width == 0 || area.height == 0 {
            return Ok(());
        }
        let row = overlay_lane_row(area.height);
        let width = area.width as usize;
        let mut s = String::new();
        s.push_str(&format!("\x1b[{row};1H\x1b[2K"));
        // The dev build marker takes the left of the lane; a shipped build has
        // none, so the emitted bytes are unchanged there.
        if let Some(text) = crate::marker::build_marker() {
            let text: String = text.chars().take(width).collect();
            s.push_str(&format!("\x1b[38;2;107;122;107m{text}\x1b[0m"));
        }
        if let Some(label) = hover_label {
            let max = width.saturating_sub(1 + crate::marker::reserved_cols() as usize);
            let text: String = label.chars().take(max).collect();
            let tw = text.chars().count();
            // Right-aligned, ochre, with a 1-column margin from the edge.
            let col = width.saturating_sub(tw).max(1);
            s.push_str(&format!(
                "\x1b[{row};{col}H\x1b[38;2;217;164;65m{text}\x1b[0m"
            ));
        }
        self.out.write_all(s.as_bytes())
    }

    /// Test-only entry point that skips the ratatui `Frame` entirely and
    /// drives `render_members` with a fixed `strip_w` matching the hit-test
    /// tests (200 columns). Errors are swallowed, mirroring `draw`'s
    /// tolerance for a failed frame.
    #[cfg(test)]
    pub fn draw_to_sink(&mut self, herd: &Herd, species: &[Species], theme: Theme, now_ms: u64) {
        let _ = self.render_members(herd, species, Rect::new(0, 0, 200, 10), theme, now_ms);
    }
}

impl MemberRenderer for KittyRenderer {
    /// Kitty images are drawn out of band (direct terminal escapes), not into
    /// the ratatui buffer — `frame` is only consulted for its width. A failed
    /// write degrades to a skipped frame rather than crashing the strip (the
    /// render loop already tolerates this for the half-block path).
    fn draw(
        &mut self,
        frame: &mut Frame,
        herd: &Herd,
        species: &[Species],
        theme: Theme,
        now_ms: u64,
        hover_label: Option<&str>,
    ) {
        let area = frame.area();
        let _ = self.render_members(herd, species, area, theme, now_ms);
        // Draw the name (and the temp build marker) as DIRECT terminal escapes,
        // in the same layer as the members — NOT ratatui text via `frame`. The
        // per-frame kitty image re-placement clobbers ratatui's text cells, and
        // ratatui's diff then skips redrawing "unchanged" text, so anything
        // drawn through the frame flashed on for one frame and then vanished
        // (the long-standing name-disappears bug). Writing straight to the sink
        // every frame keeps it stable, on its own dedicated top row.
        let _ = self.draw_overlay_text(area, hover_label);
    }

    /// Hit-test using the same visible set as `render_members`. A member's hit range
    /// is the on-screen span of its sprite's *opaque* pixels (transparent frame
    /// padding is excluded, so hover matches the visible sheep), converted from
    /// pixels to cells. We iterate in the SAME z-order `render_members` draws
    /// (priority-sorted, stable) and let the LAST covering member win — the one
    /// drawn on top — so hover selects the sprite that is visually in front when
    /// members overlap, instead of one hidden behind it. `now_ms` must match the
    /// value passed to `draw` this frame, so the hit region lines up with what
    /// was actually placed.
    fn member_at_column(
        &self,
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
        // Match render_members' z-order exactly: lowest priority first, so the
        // topmost (on-top) member is the LAST one covering the column.
        let mut order = visible;
        order.sort_by_key(|&i| priority(herd.members[i].status));

        let x = col as i32;
        // The member footprint depends on the pane height (rows -> cols); use the
        // last drawn area so the hit range matches what was placed this frame.
        let pane_h = self.last_area.map(|a| a.height).unwrap_or(8);
        let rows = member_rows(pane_h);
        let mut best: Option<usize> = None;
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
            let animated = animate(
                &member.terminal_id,
                member.status,
                state,
                now_ms,
                member.anchor,
            );
            let fr = &state.frames[animated.frame_index];
            // The image occupies `cols` cells from the member's x; the visible
            // sprite is the opaque span scaled into those cols, so hover
            // matches the sheep, not its transparent padding.
            let cols = member_cols(rows, fr.w, fr.h) as usize;
            let (lo, hi) = opaque_col_span(fr, animated.facing_left).unwrap_or((0, fr.w));
            // Round each opaque edge to the nearest cell (rather than
            // floor-left/ceil-right) so the hit region hugs the visible sprite
            // instead of over-reaching into a barely-touched edge cell.
            let left_cell = (lo * cols + fr.w / 2) / fr.w;
            let right_cell = ((hi * cols + fr.w / 2) / fr.w).max(left_cell + 1);
            let left = (animated.x_fraction * max_x).round() as i32;
            if x >= left + left_cell as i32 && x < left + right_cell as i32 {
                // Later in draw order = drawn on top → overwrite so the
                // frontmost covering member wins.
                best = Some(i);
            }
        }
        best
    }

    /// Release the images and placements *this pane* transmitted (clean exit),
    /// one `d=I` per owned id. A quitting pane must leave every other pane's
    /// images alone (issue #28).
    fn teardown(&mut self) -> io::Result<()> {
        self.free_all_images()
    }

    fn backend_name(&self) -> &'static str {
        "kitty"
    }
}

/// A `Vec<u8>`-backed [`Write`] sink shared between the renderer and the test
/// that drives it: cloning shares the same underlying buffer (`Arc<Mutex<_>>`),
/// so escapes written through a `KittyRenderer` built with one clone are
/// visible via `.take()` on another.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct SharedSink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

#[cfg(test)]
impl SharedSink {
    /// Drain and return everything written so far as a `String`, clearing the
    /// buffer so the next `.take()` only sees new writes.
    pub fn take(&self) -> String {
        let mut buf = self.0.lock().unwrap();
        let s = String::from_utf8_lossy(&buf).into_owned();
        buf.clear();
        s
    }
}

#[cfg(test)]
impl Write for SharedSink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
impl KittyRenderer {
    /// Test constructor: boxes a clone of `sink` as the write target so the
    /// caller's `sink` handle keeps observing everything written.
    pub fn for_test(sink: SharedSink, scale: usize) -> Self {
        Self::for_test_in_block(sink, scale, 0)
    }

    /// Test constructor for a specific id block — one renderer per block stands
    /// in for one pane per process, which is the only way to observe the
    /// cross-pane properties from a single test process.
    pub fn for_test_in_block(sink: SharedSink, scale: usize, block: u32) -> Self {
        Self::with_image_ids(scale, Box::new(sink), ImageIds::for_block(block))
    }

    /// The id block this renderer allocates from, so a test can assert that an
    /// id it observed belongs to this pane and not the other one.
    pub fn image_ids(&self) -> ImageIds {
        self.image_ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
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

    fn one_working_herd() -> Herd {
        let mut h = Herd::new();
        h.members.push(Member::new(
            "t1".into(),
            identity_for("t1", 1),
            AgentStatus::Working,
        ));
        h
    }

    #[test]
    fn draw_transmits_then_places_and_second_frame_reuses_the_image() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_working_herd();
        r.draw_to_sink(&herd, &species, Theme::Dark, 0); // test-only wrapper (no ratatui)
        let first = sink.take();
        assert!(first.contains("a=t"), "first draw transmits the image");
        assert!(first.contains("a=p"), "and places it");
        r.draw_to_sink(&herd, &species, Theme::Dark, 0);
        let second = sink.take();
        assert!(
            !second.contains("a=t"),
            "same frame reuses the cached image (no re-transmit)"
        );
        assert!(second.contains("a=p"), "still re-places");
        assert!(second.contains("a=d"), "and deletes the previous placement");
    }

    /// The image ids named by every kitty command in `out` whose control block
    /// carries all of `fields` (e.g. `["a=d", "d=I"]` for a data-freeing
    /// delete). Continuation chunks of a chunked transmit carry no `a=`/`i=`
    /// and are skipped.
    fn ids_where(out: &str, fields: &[&str]) -> Vec<u32> {
        out.split("\x1b_G")
            .skip(1)
            .filter_map(|chunk| chunk.split_once("\x1b\\").map(|(body, _)| body))
            .map(|body| body.split_once(';').map_or(body, |(control, _)| control))
            .filter(|control| {
                fields
                    .iter()
                    .all(|f| control.split(',').any(|kv| kv == *f))
            })
            .filter_map(|control| {
                control
                    .split(',')
                    .find_map(|kv| kv.strip_prefix("i="))
                    .and_then(|v| v.parse().ok())
            })
            .collect()
    }

    fn transmitted_image_ids(out: &str) -> Vec<u32> {
        ids_where(out, &["a=t"])
    }

    /// Ids whose *data* was freed (`d=I`), as opposed to placements taken off
    /// screen (`d=i`).
    fn freed_image_ids(out: &str) -> Vec<u32> {
        ids_where(out, &["a=d", "d=I"])
    }

    fn placed_image_ids(out: &str) -> Vec<u32> {
        ids_where(out, &["a=p"])
    }

    fn sorted(mut ids: Vec<u32>) -> Vec<u32> {
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    #[test]
    fn resize_purges_and_retransmits() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_working_herd();
        let _ = r.render_members(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, 0);
        let transmitted = sorted(transmitted_image_ids(&sink.take()));
        assert!(!transmitted.is_empty(), "the first frame transmits");
        // Same area: image stays cached, no re-transmit.
        let _ = r.render_members(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, 0);
        assert!(
            !sink.take().contains("a=t"),
            "unchanged area reuses the cache"
        );
        // Changed area (resize): purge everything and re-transmit fresh. The
        // purge names this pane's own ids — `d=A` would take every other pane's
        // images with it (issue #28).
        let _ = r.render_members(&herd, &species, Rect::new(0, 0, 120, 8), Theme::Dark, 0);
        let out = sink.take();
        assert!(
            !out.contains("d=A"),
            "resize must never emit the terminal-global delete: {out:?}"
        );
        assert_eq!(
            sorted(freed_image_ids(&out)),
            transmitted,
            "resize frees exactly the ids this pane had transmitted"
        );
        assert!(out.contains("a=t"), "resize re-transmits the image");
    }

    #[test]
    fn teardown_frees_this_panes_own_ids_and_never_the_whole_terminal() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        r.draw_to_sink(&one_idle_herd(), &species, Theme::Dark, 0); // member + Zz icon
        let transmitted = sorted(transmitted_image_ids(&sink.take()));
        assert_eq!(transmitted.len(), 2, "a member image and an icon image");
        r.teardown().unwrap();
        let out = sink.take();
        assert!(
            !out.contains("d=A"),
            "teardown must never emit the terminal-global delete: {out:?}"
        );
        assert_eq!(
            sorted(freed_image_ids(&out)),
            transmitted,
            "teardown frees every image this pane transmitted, by id"
        );
    }

    #[test]
    fn member_size_stays_small_and_keeps_aspect() {
        // Rows are capped small even in a very tall pane, and never below 2.
        // Only 1 row is reserved off the top — the caption and the icon/hat
        // share that single lane (column-separated), exactly like the
        // half-block renderer's `lane_y` — so a 4-row pane still gets 3 member
        // rows, not 2.
        assert_eq!(member_rows(3), 2);
        assert_eq!(member_rows(4), 3);
        assert_eq!(member_rows(6), 4);
        assert_eq!(member_rows(40), 4);
        // Cols preserve the sprite aspect (16x14 -> ~2.3 cols per row).
        assert_eq!(member_cols(3, 16, 14), 7); // round(3 * 16/14 * 2.1)
        assert_eq!(member_cols(4, 16, 14), 10);
    }

    #[test]
    fn member_crop_keeps_hat_and_hop_headroom_above_the_sprite() {
        // The displayed window is TOP_HEADROOM rows taller than the sprite,
        // with that headroom ABOVE it, so the baked-in hat and the walking hop
        // (up to 2px) never clip the head — the regression this whole change
        // fixes. Verify across the entire hop range (dy 0.0 -> -2.0).
        let (body_pad, scale, w, sprite_h) = (MOTION_PAD + HAT_H, 7usize, 16usize, 14usize);
        let win_h = (sprite_h + TOP_HEADROOM) * scale;
        // Topmost hat pixel row, and the row just past the feet, in padded-canvas
        // pixels — the window must always straddle both.
        let hat_top = ((body_pad - HAT_H) * scale) as u32;
        let feet = ((body_pad + sprite_h) * scale) as u32;
        let canvas_h = ((sprite_h + 2 * body_pad) * scale) as u32;
        for tenths in 0..=20 {
            let dy = -(tenths as f32) / 10.0; // 0.0 down to -2.0
            let c = member_crop(body_pad, TOP_HEADROOM, scale, w, sprite_h, 0.0, dy);
            assert_eq!(c.h as usize, win_h, "window is sprite + headroom tall");
            assert!(
                c.y <= hat_top,
                "dy={dy}: window top {} clipped the hat/head (hat top {hat_top})",
                c.y
            );
            assert!(
                c.y + c.h >= feet,
                "dy={dy}: window bottom {} clipped the feet ({feet})",
                c.y + c.h
            );
            assert!(
                c.y + c.h <= canvas_h,
                "dy={dy}: window read past the padded canvas ({canvas_h})"
            );
        }
    }

    #[test]
    fn member_row_bottom_aligns_with_no_gap_at_the_pane_floor() {
        for (pane_h, rows) in [(6, 4u16), (10, 4), (4, 2), (3, 2)] {
            let row = member_row(pane_h, rows);
            assert_eq!(
                row + rows as i32 - 1,
                pane_h,
                "the member's last occupied row must be the pane's own last row \
                 (pane_h={pane_h}, rows={rows}, row={row})"
            );
        }
    }

    #[test]
    fn overlay_lane_row_is_always_above_the_member_body_never_inside_it() {
        for pane_h in 3..40u16 {
            let rows = member_rows(pane_h);
            let row = member_row(pane_h as i32, rows);
            let lane = overlay_lane_row(pane_h);
            assert!(
                (lane as i32) < row,
                "pane_h={pane_h}: overlay lane {lane} must sit above the member's own top row {row}"
            );
        }
    }

    #[test]
    fn draw_writes_the_hover_caption_as_a_direct_escape_on_the_name_row() {
        // The caption is written straight to the sink (same layer as the members),
        // NOT into the ratatui frame — ratatui text gets clobbered by the
        // per-frame kitty re-placement and never redrawn, which made the name
        // flash on then vanish. It targets the 1-indexed name row for this pane.
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_working_herd();
        let pane_h = 10u16;
        let mut terminal = Terminal::new(TestBackend::new(40, pane_h)).unwrap();
        terminal
            .draw(|f| {
                MemberRenderer::draw(&mut r, f, &herd, &species, Theme::Dark, 0, Some("agent-x"))
            })
            .unwrap();
        let out = sink.take();
        assert!(
            out.contains("agent-x"),
            "the caption is emitted as a direct escape: {out:?}"
        );
        let row = overlay_lane_row(pane_h);
        assert!(
            out.contains(&format!("\x1b[{row};")),
            "the caption is positioned on the 1-indexed name row {row}"
        );
    }

    /// The kitty path bypasses ratatui, so the marker has to be emitted as its
    /// own escape at column 1 of the same lane the caption uses.
    #[test]
    #[cfg(feature = "dev-marker")]
    fn draw_writes_the_build_marker_at_column_one_of_the_name_row() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_working_herd();
        let pane_h = 10u16;
        let mut terminal = Terminal::new(TestBackend::new(80, pane_h)).unwrap();
        terminal
            .draw(|f| MemberRenderer::draw(&mut r, f, &herd, &species, Theme::Dark, 0, None))
            .unwrap();
        let out = sink.take();
        let marker = crate::marker::build_marker().unwrap();
        assert!(out.contains(marker), "the marker is emitted: {out:?}");
        let row = overlay_lane_row(pane_h);
        assert!(
            out.contains(&format!("\x1b[{row};1H")),
            "the marker sits at column 1 of the name row {row}"
        );
    }

    /// The shipped binary must not carry the marker at all — this is the test
    /// that would catch it leaking out of the `dev-marker` feature.
    #[test]
    #[cfg(not(feature = "dev-marker"))]
    fn a_shipped_build_emits_no_build_marker_on_the_name_row() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_working_herd();
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        terminal
            .draw(|f| MemberRenderer::draw(&mut r, f, &herd, &species, Theme::Dark, 0, None))
            .unwrap();
        let out = sink.take();
        assert!(
            !out.contains(env!("CARGO_PKG_VERSION")),
            "no build identity in a shipped build: {out:?}"
        );
    }

    #[test]
    fn draw_emits_no_caption_escape_when_nothing_is_hovered() {
        // With no hover label the name row still gets cleared + the marker, but
        // no ochre caption text — so a stale name never lingers.
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_working_herd();
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|f| MemberRenderer::draw(&mut r, f, &herd, &species, Theme::Dark, 0, None))
            .unwrap();
        let out = sink.take();
        // The ochre caption color (217;164;65) is only emitted for a real label.
        assert!(
            !out.contains("38;2;217;164;65"),
            "no caption color is emitted when nothing is hovered: {out:?}"
        );
    }

    #[test]
    fn opaque_col_span_trims_transparent_padding() {
        let species = parse_species(BLOB).unwrap();
        let fr = &species.states[&AgentStatus::Working].frames[0];
        // working frame `MM../MMM./M##./.MM.`: cols 0,1,2 opaque, col 3 empty.
        assert_eq!(opaque_col_span(fr, false), Some((0, 3)));
        // flipped: drawn col d shows source (w-1-d), so opaque source cols
        // 0,1,2 land on drawn cols 1,2,3.
        assert_eq!(opaque_col_span(fr, true), Some((1, 4)));
    }

    #[test]
    fn hit_test_uses_the_cell_footprint() {
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_working_herd();
        let mut r = KittyRenderer::for_test(SharedSink::default(), 4);
        r.draw_to_sink(&herd, &species, Theme::Dark, 0); // populates last_area for member_rows
        let hit = (0..200u16).find_map(|c| r.member_at_column(&herd, &species, 200, c, 0));
        assert_eq!(hit, Some(0), "some column under the member hits it");
        assert_eq!(
            r.member_at_column(&herd, &species, 200, 200, 0),
            None,
            "column past the strip's edge is empty"
        );
    }

    fn one_idle_herd() -> Herd {
        let mut h = Herd::new();
        h.members.push(Member::new(
            "t1".into(),
            identity_for("t1", 1),
            AgentStatus::Idle,
        ));
        h
    }

    #[test]
    fn icon_crop_never_pans_into_the_bitmap_across_the_full_wave() {
        // Regression: the displayed icon crop is `ICON_MARGIN` icon-pixels
        // larger than the bitmap on every side so the glyph doesn't fill its
        // cell edge to edge. That crop must center on the bitmap at rest and
        // never pan far enough to cut into it — the earlier version passed
        // `ICON_PAD` (instead of `ICON_PAD - ICON_MARGIN`) as the centering
        // reference, which put zero margin above/left of the bitmap at rest,
        // so the wave's rise cropped straight into the glyph's top row.
        let scale = 7; // production default (`Config::default().member_scale`)
        for kind in [IconKind::Sleep, IconKind::Alert, IconKind::Question] {
            let (iw, ih) = icon_size(kind);
            let canvas_w = (iw + 2 * ICON_PAD) * scale;
            let canvas_h = (ih + 2 * ICON_PAD) * scale;
            let bitmap_top = (ICON_PAD * scale) as u32;
            let bitmap_left = (ICON_PAD * scale) as u32;
            let bitmap_bottom = ((ICON_PAD + ih) * scale) as u32;
            let bitmap_right = ((ICON_PAD + iw) * scale) as u32;

            for i in 0..=100 {
                let phase = i as f32 / 100.0;
                let offset = crate::anim::icon_wave_offset(phase);
                let crop = crop_rect(
                    ICON_PAD - ICON_MARGIN,
                    scale,
                    iw + ICON_MARGIN * 2,
                    ih + ICON_MARGIN * 2,
                    offset.dx,
                    offset.dy,
                );
                assert!(
                    crop.y <= bitmap_top,
                    "{kind:?} phase {phase}: crop top {} panned past the bitmap's top edge {bitmap_top}",
                    crop.y
                );
                assert!(
                    crop.x <= bitmap_left,
                    "{kind:?} phase {phase}: crop left {} panned past the bitmap's left edge {bitmap_left}",
                    crop.x
                );
                assert!(
                    crop.y + crop.h >= bitmap_bottom,
                    "{kind:?} phase {phase}: crop bottom panned past the bitmap's bottom edge"
                );
                assert!(
                    crop.x + crop.w >= bitmap_right,
                    "{kind:?} phase {phase}: crop right panned past the bitmap's right edge"
                );
                assert!(
                    crop.y + crop.h <= canvas_h as u32 && crop.x + crop.w <= canvas_w as u32,
                    "{kind:?} phase {phase}: crop read past the transmitted canvas"
                );
            }
        }
    }

    #[test]
    fn working_has_no_overlay_so_no_icon_is_transmitted() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        r.draw_to_sink(&one_working_herd(), &species, Theme::Dark, 0);
        let out = sink.take();
        // Exactly one transmit (the member's own padded image), no second icon image.
        assert_eq!(out.matches("a=t").count(), 1, "working carries no icon");
    }

    #[test]
    fn idle_transmits_and_places_both_the_member_and_its_zz_icon() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        r.draw_to_sink(&one_idle_herd(), &species, Theme::Dark, 0);
        let out = sink.take();
        assert_eq!(
            out.matches("a=t").count(),
            2,
            "the member image and the Zz icon image"
        );
        assert_eq!(
            out.matches("a=p").count(),
            2,
            "the member placement and the icon placement"
        );
        // Placements are cropped-source (x=/y=/w=/h=), not the old fixed-size form.
        assert!(out.contains("x=") && out.contains("y="));
    }

    #[test]
    fn losing_the_overlay_deletes_the_stale_icon_placement() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let _ = r.render_members(
            &one_idle_herd(),
            &species,
            Rect::new(0, 0, 200, 10),
            Theme::Dark,
            0,
        );
        let _ = sink.take();
        // Same member, now working (no overlay): the old Zz icon placement must
        // be torn down, not left as a ghost badge.
        let _ = r.render_members(
            &one_working_herd(),
            &species,
            Rect::new(0, 0, 200, 10),
            Theme::Dark,
            0,
        );
        let out = sink.take();
        assert!(
            out.contains("a=d"),
            "the stale icon placement is deleted when the status stops having an overlay"
        );
        // Only one icon was ever transmitted (the Zz from the first, idle
        // draw) — losing the overlay must not transmit a fresh icon image.
        assert_eq!(
            out.matches("a=t").count(),
            1,
            "only the new working member frame is transmitted, no icon"
        );
    }

    #[test]
    fn blocked_member_placement_pans_as_time_advances() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        herd.members.push(Member::new(
            "t1".into(),
            identity_for("t1", 1),
            AgentStatus::Blocked,
        ));

        // Isolate the MEMBER's placement command (z=0; the icon badge places at
        // z=1000+ and would otherwise pollute the comparison).
        let member_placement = |out: &str| -> String {
            out.split("\x1b_G")
                .find(|chunk| chunk.contains("a=p") && chunk.contains(",z=0,"))
                .expect("the member's own placement command")
                .to_string()
        };
        let y_field = |chunk: &str| -> String {
            chunk
                .split(',')
                .find(|p| p.starts_with("y="))
                .expect("a y= field")
                .to_string()
        };

        let _ = r.render_members(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, 0);
        let y0 = y_field(&member_placement(&sink.take()));

        // Some particular pair of instants could coincidentally round to the
        // same pixel; scan a spread of them so the test isn't tied to one
        // sample landing on a flat spot in the bounce curve.
        let panned = (10..500u64).step_by(10).any(|ms| {
            let _ = r.render_members(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, ms);
            y_field(&member_placement(&sink.take())) != y0
        });
        assert!(
            panned,
            "bounce motion must pan the crop window as time advances"
        );
    }

    fn one_focused_working_herd() -> Herd {
        let mut h = Herd::new();
        let mut member = Member::new("t1".into(), identity_for("t1", 1), AgentStatus::Working);
        member.focused = true;
        h.members.push(member);
        h
    }

    fn one_focused_idle_herd() -> Herd {
        let mut h = Herd::new();
        let mut member = Member::new("t1".into(), identity_for("t1", 1), AgentStatus::Idle);
        member.focused = true;
        h.members.push(member);
        h
    }

    const HAT_OUTLINE_RGB: (u8, u8, u8) = (0x20, 0x18, 0x18);
    const HAT_FILL_RGB: (u8, u8, u8) = (0xd6, 0x2b, 0x2b);

    /// Decode the `occurrence`-th (0-indexed) transmitted image (`a=t`) in
    /// `out` into `(width, height, rgba bytes)`. Assumes one APC chunk per
    /// transmission — true at the tiny `scale=1` these decode-based tests
    /// use, comfortably under the protocol's 4096-char chunk limit.
    fn decode_transmit_at(out: &str, occurrence: usize) -> (usize, usize, Vec<u8>) {
        let chunk = out
            .split("\x1b_G")
            .filter(|c| c.starts_with("a=t"))
            .nth(occurrence)
            .expect("a transmit chunk at this occurrence");
        // Bound to this APC's own terminator — splitting on `\x1b_G` alone
        // leaves whatever follows (e.g. the next escape's `\x1b[{row};{col}H`
        // CUP sequence) dangling on the end of this piece.
        let end = chunk.find("\x1b\\").expect("APC terminator");
        let body = &chunk[..end];
        let (control, payload) = body.split_once(';').expect("control;payload");
        let field = |key: &str| -> usize {
            control
                .split(',')
                .find_map(|kv| kv.strip_prefix(key))
                .unwrap_or_else(|| panic!("{key} field in {control:?}"))
                .parse()
                .unwrap()
        };
        let (w, h) = (field("s="), field("v="));
        (w, h, crate::base64::decode(payload))
    }

    fn contains_rgb(px: &[u8], target: (u8, u8, u8)) -> bool {
        px.chunks(4)
            .any(|p| p[3] == 255 && (p[0], p[1], p[2]) == target)
    }

    #[test]
    fn focused_working_member_bakes_a_visible_hat_into_its_own_image() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 1);
        let species = vec![parse_species(BLOB).unwrap()];
        r.draw_to_sink(&one_focused_working_herd(), &species, Theme::Dark, 0);
        let (_w, _h, px) = decode_transmit_at(&sink.take(), 0);
        assert!(
            contains_rgb(&px, HAT_FILL_RGB),
            "the hat's fill color is baked into the transmitted image"
        );
        assert!(
            contains_rgb(&px, HAT_OUTLINE_RGB),
            "the hat's outline color is baked into the transmitted image"
        );
    }

    #[test]
    fn focused_idle_member_bakes_a_visible_hat_despite_the_dozing_poses_top_padding() {
        // Regression: the idle "dozing" pose is normalised to the same frame
        // size as standing poses by padding blank rows on TOP (see
        // sprites/sheep.sprite's comment). A hat placed relative to the
        // member's own top cell (rather than the actual head) used to float in
        // that padding, invisible against the mostly-transparent top of the
        // image. Baking it in at the real head anchor fixes this for every
        // pose, not just standing ones.
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 1);
        let species = vec![parse_species(BLOB).unwrap()];
        r.draw_to_sink(&one_focused_idle_herd(), &species, Theme::Dark, 0);
        let (_w, _h, px) = decode_transmit_at(&sink.take(), 0);
        assert!(
            contains_rgb(&px, HAT_FILL_RGB),
            "hat fill must be visible even on the idle/dozing pose"
        );
        assert!(
            contains_rgb(&px, HAT_OUTLINE_RGB),
            "hat outline must be visible even on the idle/dozing pose"
        );
    }

    #[test]
    fn unfocused_members_image_has_no_hat_pixels() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 1);
        let species = vec![parse_species(BLOB).unwrap()];
        r.draw_to_sink(&one_working_herd(), &species, Theme::Dark, 0);
        let (_w, _h, px) = decode_transmit_at(&sink.take(), 0);
        assert!(
            !contains_rgb(&px, HAT_FILL_RGB),
            "an unfocused member's image must carry no hat pixels"
        );
    }

    #[test]
    fn gaining_focus_retransmits_a_distinct_hat_bearing_image() {
        // The cache key includes `focused` (the hat is baked directly into
        // the image), so toggling focus must retransmit and re-place —
        // reusing the unfocused image would silently hide the hat.
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        r.draw_to_sink(&one_working_herd(), &species, Theme::Dark, 0);
        let _ = sink.take();
        r.draw_to_sink(&one_focused_working_herd(), &species, Theme::Dark, 0);
        let out = sink.take();
        assert_eq!(
            out.matches("a=t").count(),
            1,
            "gaining focus must transmit a fresh (hat-bearing) image, not reuse the unfocused one"
        );
        assert!(
            out.contains("a=d"),
            "the stale unfocused placement is deleted"
        );
    }

    #[test]
    fn focused_idle_member_still_shows_its_zz_icon_alongside_the_baked_in_hat() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        r.draw_to_sink(&one_focused_idle_herd(), &species, Theme::Dark, 0);
        let out = sink.take();
        assert_eq!(
            out.matches("a=t").count(),
            2,
            "the member's own (hat-bearing) frame and the Zz icon — no separate hat image"
        );
        assert_eq!(
            out.matches("a=p").count(),
            2,
            "the member placement and the icon placement — no separate hat placement"
        );
    }

    #[test]
    fn hat_pans_with_the_members_own_motion_since_its_baked_into_the_same_image() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_focused_working_herd();
        let member_placement = |out: &str| -> String {
            out.split("\x1b_G")
                .find(|chunk| chunk.contains("a=p") && chunk.contains(",z=0,"))
                .expect("the member's own placement command")
                .to_string()
        };
        let y_field = |chunk: &str| -> String {
            chunk
                .split(',')
                .find(|p| p.starts_with("y="))
                .expect("a y= field")
                .to_string()
        };
        let _ = r.render_members(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, 0);
        let y0 = y_field(&member_placement(&sink.take()));
        let panned = (10..500u64).step_by(10).any(|ms| {
            let _ = r.render_members(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, ms);
            y_field(&member_placement(&sink.take())) != y0
        });
        assert!(
            panned,
            "the combined body+hat image must still pan with motion"
        );
    }

    #[test]
    fn departed_members_placement_is_deleted() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let _ = r.render_members(
            &one_focused_working_herd(),
            &species,
            Rect::new(0, 0, 200, 10),
            Theme::Dark,
            0,
        );
        let _ = sink.take();
        // The member is gone entirely (empty herd): its placement must not
        // linger as a ghost image.
        let _ = r.render_members(
            &Herd::new(),
            &species,
            Rect::new(0, 0, 200, 10),
            Theme::Dark,
            0,
        );
        let out = sink.take();
        assert!(
            out.contains("a=d"),
            "the departed member's placement is deleted"
        );
    }

    #[test]
    fn anchored_member_placement_column_stays_fixed_as_time_advances() {
        // The kitty path threads `member.anchor` into `animate` exactly like the
        // half-block path — a frozen member's cursor column must not drift.
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        let mut member = Member::new("t1".into(), identity_for("t1", 1), AgentStatus::Idle);
        member.anchor = Some(crate::motion::Anchor {
            frozen_x: 0.4,
            settled_at_ms: 0,
        });
        herd.members.push(member);

        // The member body's cursor-move escape is written before its placement
        // (and before the overlay icon's own cursor move), so the first
        // `\x1b[...H` in the frame is the member's.
        let cursor_pos = |out: &str| -> String {
            let start = out.find("\x1b[").expect("a cursor-move escape") + 2;
            let end = out[start..].find('H').expect("terminated cursor move") + start;
            out[start..end].to_string()
        };

        let _ = r.render_members(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, 0);
        let pos0 = cursor_pos(&sink.take());
        let _ = r.render_members(
            &herd,
            &species,
            Rect::new(0, 0, 200, 10),
            Theme::Dark,
            60_000,
        );
        let pos1 = cursor_pos(&sink.take());
        assert_eq!(
            pos0, pos1,
            "an anchored member's column must stay fixed regardless of elapsed time"
        );
    }

    // ---- Cross-pane properties ----------------------------------------
    //
    // Every strip pane is its own process writing into ONE outer terminal, so
    // ids, deletes and image memory are shared. A test that drives a single
    // renderer cannot see any of that, so each test below drives TWO renderers
    // against separate sinks — pane A and pane B — exactly as two panes would.

    /// Two panes drawing the same herd, each with its own id block.
    fn two_panes(scale: usize) -> (SharedSink, KittyRenderer, SharedSink, KittyRenderer) {
        let sink_a = SharedSink::default();
        let sink_b = SharedSink::default();
        // Distinct blocks stand in for distinct processes; `for_process`
        // derives the block from the pid, which is identical inside one test.
        let a = KittyRenderer::for_test_in_block(sink_a.clone(), scale, 11);
        let b = KittyRenderer::for_test_in_block(sink_b.clone(), scale, 12);
        (sink_a, a, sink_b, b)
    }

    #[test]
    fn two_panes_never_transmit_under_the_same_image_id() {
        // The bug: `next_id` started at 1 in every process, so pane B's first
        // transmit replaced the pixels behind pane A's `i=1` — A then placed
        // its cached id and drew B's sprite (issue #29).
        let (sink_a, mut a, sink_b, mut b) = two_panes(4);
        let species = vec![parse_species(BLOB).unwrap()];
        // Idle draws a member image plus an icon image, so several ids each.
        a.draw_to_sink(&one_idle_herd(), &species, Theme::Dark, 0);
        b.draw_to_sink(&one_idle_herd(), &species, Theme::Dark, 0);
        let ids_a = sorted(transmitted_image_ids(&sink_a.take()));
        let ids_b = sorted(transmitted_image_ids(&sink_b.take()));
        assert!(ids_a.len() >= 2 && ids_b.len() >= 2, "both panes transmitted");
        for id in &ids_a {
            assert!(
                !ids_b.contains(id),
                "id {id} was transmitted by both panes into the shared namespace"
            );
        }
        // And each pane's ids come from its own block, so the disjointness
        // holds for every id either pane will ever allocate — not just these.
        assert!(ids_a.iter().all(|&id| a.image_ids().contains(id)));
        assert!(ids_b.iter().all(|&id| b.image_ids().contains(id)));
        assert!(ids_a.iter().all(|&id| !b.image_ids().contains(id)));
    }

    #[test]
    fn a_pane_tearing_down_leaves_the_other_panes_cached_images_intact() {
        // The bug: teardown emitted `d=A`, which is terminal-global — it freed
        // pane B's images while B's cache still mapped to those dead ids, so B
        // went permanently blank, placing gone ids forever (issue #28).
        let (sink_a, mut a, sink_b, mut b) = two_panes(4);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_idle_herd();
        a.draw_to_sink(&herd, &species, Theme::Dark, 0);
        b.draw_to_sink(&herd, &species, Theme::Dark, 0);
        let _ = sink_a.take();
        let ids_b = sorted(transmitted_image_ids(&sink_b.take()));

        a.teardown().unwrap();
        let out_a = sink_a.take();
        assert!(
            !out_a.contains("d=A"),
            "a quitting pane must not free the whole terminal: {out_a:?}"
        );
        for id in freed_image_ids(&out_a) {
            assert!(
                !ids_b.contains(&id),
                "pane A freed id {id}, which belongs to pane B"
            );
            assert!(a.image_ids().contains(id), "A freed an id outside its block");
        }

        // B's cached ids are still live in the terminal, so its next frame is a
        // pure re-place: no re-transmit needed, and the ids it places are the
        // same ones it transmitted before A quit.
        b.draw_to_sink(&herd, &species, Theme::Dark, 0);
        let out_b = sink_b.take();
        assert!(
            !out_b.contains("a=t"),
            "pane B's cache must survive pane A's teardown"
        );
        for id in placed_image_ids(&out_b) {
            assert!(
                ids_b.contains(&id),
                "pane B placed id {id}, which it never transmitted"
            );
        }
    }

    #[test]
    fn a_resize_in_one_pane_frees_only_that_panes_images() {
        // Same shape as teardown, but the far more common trigger: any window
        // resize purged the whole terminal, blanking every other pane.
        let (sink_a, mut a, sink_b, mut b) = two_panes(4);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_idle_herd();
        let _ = a.render_members(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, 0);
        let _ = b.render_members(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, 0);
        let _ = sink_a.take();
        let ids_b = sorted(transmitted_image_ids(&sink_b.take()));

        let _ = a.render_members(&herd, &species, Rect::new(0, 0, 120, 8), Theme::Dark, 0);
        let out_a = sink_a.take();
        assert!(!out_a.contains("d=A"), "resize is not terminal-global");
        for id in freed_image_ids(&out_a) {
            assert!(
                !ids_b.contains(&id),
                "pane A's resize freed pane B's image {id}"
            );
        }

        let _ = b.render_members(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, 0);
        let out_b = sink_b.take();
        assert!(
            !out_b.contains("a=t"),
            "pane B must not have to re-transmit because pane A resized"
        );
        assert!(
            !placed_image_ids(&out_b).is_empty(),
            "pane B keeps drawing from its own cache"
        );
    }

    #[test]
    fn no_command_in_a_panes_whole_lifecycle_is_terminal_global() {
        // A blanket scan over transmit, re-place, status change, departure,
        // resize and teardown: nothing may reach outside this pane's own ids.
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test_in_block(sink.clone(), 4, 5);
        let species = vec![parse_species(BLOB).unwrap()];
        let mut all = String::new();
        for (herd, area, ms) in [
            (one_idle_herd(), Rect::new(0, 0, 200, 10), 0u64),
            (one_working_herd(), Rect::new(0, 0, 200, 10), 100),
            (one_focused_idle_herd(), Rect::new(0, 0, 120, 8), 200),
            (Herd::new(), Rect::new(0, 0, 120, 8), 300),
        ] {
            let _ = r.render_members(&herd, &species, area, Theme::Dark, ms);
            all.push_str(&sink.take());
        }
        r.teardown().unwrap();
        all.push_str(&sink.take());
        assert!(
            !all.contains("d=A") && !all.contains("d=a"),
            "no delete may be terminal-global: {all:?}"
        );
        let ids = r.image_ids();
        for id in transmitted_image_ids(&all)
            .into_iter()
            .chain(freed_image_ids(&all))
            .chain(placed_image_ids(&all))
        {
            assert!(ids.contains(id), "id {id} is outside this pane's block");
        }
    }

    #[test]
    fn placements_do_not_consume_the_panes_image_ids() {
        // Placement ids used to come off the same counter as image ids, so a
        // pane burned ~one id per member per frame and would have run through
        // any block it was given within minutes.
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test_in_block(sink.clone(), 4, 6);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_working_herd();
        for ms in (0..2000).step_by(50) {
            let _ = r.render_members(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, ms);
        }
        let transmitted = sorted(transmitted_image_ids(&sink.take()));
        let base = r.image_ids().base();
        assert!(
            transmitted.iter().all(|&id| id < base + 8),
            "40 frames must not walk the image-id space forward: {transmitted:?}"
        );
    }

    // ---- Image-data lifetime (issue #30) --------------------------------

    #[test]
    fn an_image_that_stops_being_placed_is_freed_and_retransmitted_later() {
        // `d=i` (delete_placement) only takes an image off screen; its pixels
        // stayed resident in the terminal forever. Once a member is gone for
        // the TTL its image data must actually be handed back.
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let area = Rect::new(0, 0, 200, 10);
        let _ = r.render_members(&one_working_herd(), &species, area, Theme::Dark, 0);
        let transmitted = sorted(transmitted_image_ids(&sink.take()));

        // The member departs; its image is now unplaced but still resident.
        let empty = Herd::new();
        let mut freed = Vec::new();
        for _ in 0..=IMAGE_TTL_FRAMES {
            let _ = r.render_members(&empty, &species, area, Theme::Dark, 0);
            freed.extend(freed_image_ids(&sink.take()));
        }
        assert_eq!(
            sorted(freed),
            transmitted,
            "an image unplaced for the TTL has its data freed by id"
        );

        // Freed means gone from the terminal: the cache must not keep claiming
        // it, or the member would come back as a placement of a dead id.
        let _ = r.render_members(&one_working_herd(), &species, area, Theme::Dark, 0);
        assert!(
            sink.take().contains("a=t"),
            "the member re-transmits after its image was freed"
        );
    }

    #[test]
    fn an_image_still_on_screen_is_never_freed_however_long_it_is_drawn() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let area = Rect::new(0, 0, 200, 10);
        let herd = one_working_herd();
        let mut freed = Vec::new();
        // Frozen `now_ms` (reduced motion) holds the member on one animation
        // frame, so this is the same image every time — well past the TTL.
        for _ in 0..IMAGE_TTL_FRAMES + 50 {
            let _ = r.render_members(&herd, &species, area, Theme::Dark, 0);
            freed.extend(freed_image_ids(&sink.take()));
        }
        assert!(
            freed.is_empty(),
            "a continuously drawn image must never be freed: {freed:?}"
        );
    }

    #[test]
    fn evict_from_frees_stale_entries_and_keeps_the_current_frames() {
        let mut cache: HashMap<u8, Cached> = HashMap::new();
        cache.insert(1, Cached { id: 10, last_used: 0 });
        cache.insert(
            2,
            Cached {
                id: 20,
                last_used: IMAGE_TTL_FRAMES,
            },
        );
        let freed = evict_from(&mut cache, IMAGE_TTL_FRAMES + 1);
        assert_eq!(freed, vec![10], "only the entry past its TTL is freed");
        assert!(cache.contains_key(&2), "the fresh entry stays cached");
    }

    #[test]
    fn evict_from_caps_the_cache_without_touching_this_frames_images() {
        let mut cache: HashMap<u32, Cached> = HashMap::new();
        // One live entry on the current frame, plus a burst of older ones —
        // all still inside the TTL, so only the cap can reclaim them.
        for i in 0..(MAX_CACHED_IMAGES as u32 + 40) {
            cache.insert(
                i,
                Cached {
                    id: 1000 + i,
                    last_used: 1 + u64::from(i),
                },
            );
        }
        let frame = MAX_CACHED_IMAGES as u64 + 40;
        let live = *cache.get(&(MAX_CACHED_IMAGES as u32 + 39)).unwrap();
        assert_eq!(live.last_used, frame, "that entry is this frame's");
        let freed = evict_from(&mut cache, frame);
        assert_eq!(cache.len(), MAX_CACHED_IMAGES, "the cache is capped");
        assert_eq!(freed.len(), 40, "the oldest entries above the cap are freed");
        assert!(
            !freed.contains(&live.id),
            "an image placed this frame is never evicted"
        );
    }
}
