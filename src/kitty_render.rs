//! Kitty graphics protocol backend for [`PetRenderer`]: transmits each
//! distinct sprite frame once and caches the image id, places/re-places it at
//! the pet's cell every frame, and deletes placements for pets that departed
//! or fell out of the visible set. Escapes are written to an injected
//! `io::Write` sink (real stdout in production, a `Vec<u8>`-backed sink in
//! tests) so the encoding is unit-testable without a real terminal.

use std::collections::HashMap;
use std::io::{self, Write};

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::agent::AgentStatus;
use crate::anim::{Overlay, OverlayColor};
use crate::herd::{Herd, visible_and_hidden};
use crate::icon::{IconKind, icon_size, rasterize_icon};
use crate::kitty::{Crop, delete_all, delete_placement, place_cropped, transmit_rgba};
use crate::motion::animate;
use crate::palette::{StateStyle, Theme, role_color};
use crate::pet::priority;
use crate::raster::{pad_frame, rasterize};
use crate::render::{HAT_H, HAT_W, PetRenderer, head_anchor, rasterize_hat};
use crate::sprite::{Frame as SpriteFrame, Species};

/// Sprite-pixel margin padded around a transmitted pet image, so a motion
/// offset can be animated by panning a crop window instead of retransmitting.
/// 2px comfortably covers every motion's max amplitude (breathe <=0.5, hop/
/// bounce <=1.0, sway <=1.0).
const MOTION_PAD: usize = 2;

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

/// Stacking index floor for overlay icons: always above every pet body, which
/// draw at `z = 0..order.len()`.
const Z_ICON_BASE: i32 = 1000;

/// Stacking index floor for the focus hat: always above every pet body AND
/// every overlay icon (a focused, idle pet shows both its Zz icon and its
/// hat, and the hat must win).
const Z_HAT_BASE: i32 = 100_000;

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

/// Rows a pet image occupies, derived from the pane height (reserving a
/// little headroom for the top caption/overflow lane) and capped small so
/// the pets always read as a slim strip, even in a tall pane.
fn pet_rows(pane_h: u16) -> u16 {
    pane_h.saturating_sub(1).clamp(2, 4)
}

/// Columns for a `frame_w` x `frame_h` sprite shown at `rows` rows, preserving
/// the sprite's aspect under an assumed ~1:2.1 cell width:height ratio. herdr
/// hides the real cell size, so we approximate it; the worst case is a slight
/// horizontal stretch, imperceptible for pixel art. Placing with explicit
/// `c=`/`r=` (rather than native pixels) makes the on-screen footprint exact,
/// which is what lets hover hit-testing line up with the visible sprite.
fn pet_cols(rows: u16, frame_w: usize, frame_h: usize) -> u16 {
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

/// Cache key for a transmitted image: species/status/frame/flip/hue fully
/// determine the rasterized pixels, so two pets sharing all five reuse one
/// image id instead of retransmitting.
type ImgKey = (usize, AgentStatus, usize, bool, u16);

/// Draws the herd via the kitty graphics protocol instead of ratatui cells.
/// Images are transmitted once per distinct `ImgKey` and cached; each visible
/// pet gets a placement that is re-created every frame (draw-then-delete-old,
/// to avoid a flicker where the pet briefly vanishes).
pub struct KittyRenderer {
    scale: usize,
    out: Box<dyn Write + Send>,
    cache: HashMap<ImgKey, u32>,
    /// `terminal_id -> (image_id, placement_id)` of that pet's current
    /// placement. The image id is tracked alongside the placement id (not
    /// just the placement id, per the plan's original shape) because a
    /// redraw can move the pet onto a *different* cached image — a new
    /// status, frame, or facing direction — and deleting the previous
    /// on-screen placement requires the image id it was placed under, which
    /// a `HashMap<String, u32>` of placement ids alone cannot recover.
    placements: HashMap<String, (u32, u32)>,
    /// Transmitted overlay icon images, cached by (icon kind, is-dark-theme,
    /// resolved overlay color) since an icon's pixels don't depend on
    /// species/hue/facing, but `done` and `blocked` share `IconKind::Alert`
    /// (both use `!`) and must render as distinct images (accent vs. red).
    icon_cache: HashMap<(IconKind, bool, OverlayColor), u32>,
    /// `terminal_id -> (image_id, placement_id)` of that pet's current icon
    /// placement, if its state has an overlay. Tracked separately from
    /// `placements` because a pet can lose its overlay (e.g. idle -> working)
    /// while staying visible, which must delete the icon but keep the pet.
    icon_placements: HashMap<String, (u32, u32)>,
    /// The transmitted focus-hat image id, once rasterized. Unlike pet/icon
    /// images the hat never varies (always the same red hat, no theme/hue),
    /// so a single cached id suffices.
    hat_cache: Option<u32>,
    /// `terminal_id -> (image_id, placement_id)` of that pet's current hat
    /// placement, if it's the focused pet. Tracked separately from
    /// `placements`/`icon_placements` because a pet can lose focus while
    /// staying visible, which must delete the hat but keep the pet (and any
    /// icon).
    hat_placements: HashMap<String, (u32, u32)>,
    next_id: u32,
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
    /// (`pet_rows`/`pet_cols`) at placement time.
    pub fn new(scale: usize, out: Box<dyn Write + Send>) -> Self {
        Self {
            scale,
            out,
            cache: HashMap::new(),
            placements: HashMap::new(),
            icon_cache: HashMap::new(),
            icon_placements: HashMap::new(),
            hat_cache: None,
            hat_placements: HashMap::new(),
            next_id: 1,
            last_area: None,
        }
    }

    /// Write all escapes for the current frame's visible pets to `self.out`,
    /// replicating `render::draw_herd`'s visible-set + z-order selection
    /// exactly so the kitty and half-block backends agree on what's shown.
    fn render_pets(
        &mut self,
        herd: &Herd,
        species: &[Species],
        area: Rect,
        theme: Theme,
        now_ms: u64,
    ) -> io::Result<()> {
        // On a geometry change (resize), the terminal may have dropped our
        // transmitted images. Purge everything and re-transmit fresh this
        // frame, or `place` would reference gone images and leave the strip
        // blank. The per-pet positions below already reflow to the new area.
        if self.last_area != Some(area) {
            self.out.write_all(delete_all().as_bytes())?;
            self.cache.clear();
            self.placements.clear();
            self.icon_cache.clear();
            self.icon_placements.clear();
            self.hat_cache = None;
            self.hat_placements.clear();
            self.last_area = Some(area);
        }

        let strip_w = area.width as usize;
        let pet_w = species.first().map(|s| s.size().0).unwrap_or(12);
        let max_x = (strip_w as f32 - pet_w as f32).max(0.0);
        let capacity = (strip_w / (pet_w * 3 / 4).max(1)).max(1);
        let (visible, _hidden) = visible_and_hidden(&herd.pets, capacity);

        let mut order = visible.clone();
        order.sort_by_key(|&i| priority(herd.pets[i].status));

        for (zi, &i) in order.iter().enumerate() {
            let pet = &herd.pets[i];
            let Some(sp) = species
                .get(pet.identity.species_index)
                .or_else(|| species.first())
            else {
                continue;
            };
            let Some(state) = sp.states.get(&pet.status) else {
                continue;
            };
            let animated = animate(&pet.terminal_id, pet.status, state, now_ms);
            let fr = &state.frames[animated.frame_index];
            let key: ImgKey = (
                pet.identity.species_index,
                pet.status,
                animated.frame_index,
                animated.facing_left,
                pet.identity.hue,
            );
            let image_id = match self.cache.get(&key) {
                Some(&id) => id,
                None => {
                    let style = StateStyle {
                        dim: state.dim,
                        ghost: state.ghost,
                    };
                    // Transmit a padded canvas (once per key), so this state's
                    // motion can be animated below by panning a crop window
                    // over it instead of retransmitting every frame.
                    let padded = pad_frame(fr, MOTION_PAD);
                    let rgba = rasterize(
                        &padded,
                        pet.identity.hue,
                        theme,
                        style,
                        self.scale,
                        animated.facing_left,
                    );
                    let id = self.next_id;
                    self.next_id += 1;
                    self.out
                        .write_all(transmit_rgba(id, rgba.w, rgba.h, &rgba.px).as_bytes())?;
                    self.cache.insert(key, id);
                    id
                }
            };

            // Size the pet to an explicit cell footprint (herdr hides the real
            // cell size), then bottom-anchor it so its feet rest just above the
            // caption row (the pane's last row), matching the half-block path.
            // Recomputed every frame, so a resize reflows the pets. Cursor
            // coordinates are 1-based and clamped into the pane so an edge pet
            // is never placed off-screen.
            let rows = pet_rows(area.height);
            let cols = pet_cols(rows, fr.w, fr.h);
            let pane_h = area.height as i32;
            let row = (pane_h - rows as i32).clamp(1, pane_h.max(1));
            let col = ((animated.x_fraction * max_x).round() as i32 + 1)
                .clamp(1, area.width.max(1) as i32);
            self.out
                .write_all(format!("\x1b[{row};{col}H").as_bytes())?;

            let pid = self.next_id;
            self.next_id += 1;
            // Pan the crop window by this state's motion offset (breathe/hop/
            // bounce/sway) — the same offset the half-block path bakes
            // straight into its pixel buffer — so the body actually animates
            // instead of sitting dead still.
            let crop = crop_rect(
                MOTION_PAD,
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
                .insert(pet.terminal_id.clone(), (image_id, pid))
            {
                // Draw-then-delete: the new placement is already on screen
                // before the old one disappears, so there is no blank frame.
                self.out
                    .write_all(delete_placement(old_img, old_pid).as_bytes())?;
            }

            // Overlay icon: a small pixel-art Zz/!/? floating just above the
            // pet, on its own wave motion (`pet.icon_phase`), independent of
            // the body's own state. No overlay this state -> drop any
            // lingering icon placement from a previous status.
            let glyph = match &state.overlay.kind {
                Overlay::Bubble(g) | Overlay::Badge(g) => Some(g.as_str()),
                Overlay::None => None,
            };
            match glyph.and_then(IconKind::from_glyph) {
                Some(kind) => {
                    let icon_key = (kind, theme == Theme::Dark, state.overlay.color);
                    let icon_image_id = match self.icon_cache.get(&icon_key) {
                        Some(&id) => id,
                        None => {
                            let rgba = rasterize_icon(
                                kind,
                                theme,
                                state.overlay.color,
                                self.scale,
                                ICON_PAD,
                            );
                            let id = self.next_id;
                            self.next_id += 1;
                            self.out.write_all(
                                transmit_rgba(id, rgba.w, rgba.h, &rgba.px).as_bytes(),
                            )?;
                            self.icon_cache.insert(icon_key, id);
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
                    let icon_cols = pet_cols(icon_rows, iw, ih);
                    let icon_row = row.saturating_sub(1).max(1);
                    let icon_col_max = (area.width as i32 - icon_cols as i32 + 1).max(1);
                    let icon_col =
                        (col + (cols as i32) / 2 - (icon_cols as i32) / 2).clamp(1, icon_col_max);
                    self.out
                        .write_all(format!("\x1b[{icon_row};{icon_col}H").as_bytes())?;
                    let icon_pid = self.next_id;
                    self.next_id += 1;
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
                        .insert(pet.terminal_id.clone(), (icon_image_id, icon_pid))
                    {
                        self.out
                            .write_all(delete_placement(old_img, old_pid).as_bytes())?;
                    }
                }
                None => {
                    if let Some((old_img, old_pid)) = self.icon_placements.remove(&pet.terminal_id)
                    {
                        // This status has no overlay (e.g. working) — drop any
                        // icon left over from a previous status (e.g. idle).
                        self.out
                            .write_all(delete_placement(old_img, old_pid).as_bytes())?;
                    }
                }
            }

            // Focus hat: a small red hat above the focused pet's head, panned
            // by the same motion offset as the body (`crop_rect`) so it never
            // detaches during the hop. Stacked above both the body and any
            // overlay icon (`Z_HAT_BASE`). Torn down the moment focus moves
            // away, mirroring the icon teardown just above.
            if pet.focused {
                let hat_image_id = match self.hat_cache {
                    Some(id) => id,
                    None => {
                        let rgba = rasterize_hat(self.scale, MOTION_PAD);
                        let id = self.next_id;
                        self.next_id += 1;
                        self.out
                            .write_all(transmit_rgba(id, rgba.w, rgba.h, &rgba.px).as_bytes())?;
                        self.hat_cache = Some(id);
                        id
                    }
                };
                // Same pad/offset inputs as the body's own `crop_rect` call
                // above, so the hat pans by the identical amount and never
                // drifts away from the head during motion.
                let hat_crop = crop_rect(
                    MOTION_PAD,
                    self.scale,
                    HAT_W,
                    HAT_H,
                    animated.offset.dx,
                    animated.offset.dy,
                );
                let hat_rows: u16 = 1;
                let hat_cols = pet_cols(hat_rows, HAT_W, HAT_H);
                // Center the hat over the head anchor's column (already in
                // drawn, post-flip space — see `head_anchor`), mapped from
                // sprite-pixel space into this pet's own on-screen footprint.
                let (_head_row, head_col) = head_anchor(fr, animated.facing_left);
                let head_frac = head_col as f32 / fr.w.max(1) as f32;
                let hat_row = row.saturating_sub(1).max(1);
                let hat_col_center = col + (head_frac * cols as f32).round() as i32;
                let hat_col_max = (area.width as i32 - hat_cols as i32 + 1).max(1);
                let hat_col = (hat_col_center - (hat_cols as i32) / 2).clamp(1, hat_col_max);
                self.out
                    .write_all(format!("\x1b[{hat_row};{hat_col}H").as_bytes())?;
                let hat_pid = self.next_id;
                self.next_id += 1;
                self.out.write_all(
                    place_cropped(
                        hat_image_id,
                        hat_pid,
                        hat_crop,
                        hat_cols,
                        hat_rows,
                        Z_HAT_BASE + zi as i32,
                    )
                    .as_bytes(),
                )?;
                if let Some((old_img, old_pid)) = self
                    .hat_placements
                    .insert(pet.terminal_id.clone(), (hat_image_id, hat_pid))
                {
                    self.out
                        .write_all(delete_placement(old_img, old_pid).as_bytes())?;
                }
            } else if let Some((old_img, old_pid)) = self.hat_placements.remove(&pet.terminal_id) {
                // Focus moved away from this (still-visible) pet — drop its
                // hat placement.
                self.out
                    .write_all(delete_placement(old_img, old_pid).as_bytes())?;
            }
        }

        // Any tracked pet not in the current visible set (departed, or pushed
        // out by overflow) no longer belongs on screen: drop its placement so
        // it doesn't linger as a ghost image.
        let visible_ids: std::collections::HashSet<&str> = visible
            .iter()
            .map(|&i| herd.pets[i].terminal_id.as_str())
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
        let departed_hats: Vec<String> = self
            .hat_placements
            .keys()
            .filter(|tid| !visible_ids.contains(tid.as_str()))
            .cloned()
            .collect();
        for tid in departed_hats {
            if let Some((img, pid)) = self.hat_placements.remove(&tid) {
                self.out.write_all(delete_placement(img, pid).as_bytes())?;
            }
        }

        Ok(())
    }

    /// Test-only entry point that skips the ratatui `Frame` entirely and
    /// drives `render_pets` with a fixed `strip_w` matching the hit-test
    /// tests (200 columns). Errors are swallowed, mirroring `draw`'s
    /// tolerance for a failed frame.
    #[cfg(test)]
    pub fn draw_to_sink(&mut self, herd: &Herd, species: &[Species], theme: Theme, now_ms: u64) {
        let _ = self.render_pets(herd, species, Rect::new(0, 0, 200, 10), theme, now_ms);
    }
}

impl PetRenderer for KittyRenderer {
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
        let _ = self.render_pets(herd, species, area, theme, now_ms);
        // Kitty draws pets out of band and never reserves a top lane of its
        // own for `+N` (it doesn't draw one), so the caption has the whole
        // top row to itself — no overflow width to dodge.
        crate::render::draw_caption(frame, area, area.y, hover_label, 0);
    }

    /// Hit-test using the same visible set as `render_pets`. A pet's hit range
    /// is the on-screen span of its sprite's *opaque* pixels (transparent frame
    /// padding is excluded, so hover matches the visible sheep), converted from
    /// pixels to cells. We iterate in the SAME z-order `render_pets` draws
    /// (priority-sorted, stable) and let the LAST covering pet win — the one
    /// drawn on top — so hover selects the sprite that is visually in front when
    /// pets overlap, instead of one hidden behind it. `now_ms` must match the
    /// value passed to `draw` this frame, so the hit region lines up with what
    /// was actually placed.
    fn pet_at_column(
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
        let (visible, _hidden) = visible_and_hidden(&herd.pets, capacity);
        // Match render_pets' z-order exactly: lowest priority first, so the
        // topmost (on-top) pet is the LAST one covering the column.
        let mut order = visible;
        order.sort_by_key(|&i| priority(herd.pets[i].status));

        let x = col as i32;
        // The pet footprint depends on the pane height (rows -> cols); use the
        // last drawn area so the hit range matches what was placed this frame.
        let pane_h = self.last_area.map(|a| a.height).unwrap_or(8);
        let rows = pet_rows(pane_h);
        let mut best: Option<usize> = None;
        for &i in &order {
            let pet = &herd.pets[i];
            let Some(sp) = species
                .get(pet.identity.species_index)
                .or_else(|| species.first())
            else {
                continue;
            };
            let Some(state) = sp.states.get(&pet.status) else {
                continue;
            };
            let animated = animate(&pet.terminal_id, pet.status, state, now_ms);
            let fr = &state.frames[animated.frame_index];
            // The image occupies `cols` cells from the pet's x; the visible
            // sprite is the opaque span scaled into those cols, so hover
            // matches the sheep, not its transparent padding.
            let cols = pet_cols(rows, fr.w, fr.h) as usize;
            let (lo, hi) = opaque_col_span(fr, animated.facing_left).unwrap_or((0, fr.w));
            // Round each opaque edge to the nearest cell (rather than
            // floor-left/ceil-right) so the hit region hugs the visible sprite
            // instead of over-reaching into a barely-touched edge cell.
            let left_cell = (lo * cols + fr.w / 2) / fr.w;
            let right_cell = ((hi * cols + fr.w / 2) / fr.w).max(left_cell + 1);
            let left = (animated.x_fraction * max_x).round() as i32;
            if x >= left + left_cell as i32 && x < left + right_cell as i32 {
                // Later in draw order = drawn on top → overwrite so the
                // frontmost covering pet wins.
                best = Some(i);
            }
        }
        best
    }

    /// Release all transmitted images and placements (clean exit).
    fn teardown(&mut self) -> io::Result<()> {
        self.out.write_all(delete_all().as_bytes())
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
        Self::new(scale, Box::new(sink))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
    use crate::herd::Herd;
    use crate::identity::identity_for;
    use crate::palette::Theme;
    use crate::pet::Pet;
    use crate::sprite::parse_species;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const BLOB: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/sprites/test-blob.sprite"
    ));

    fn one_working_herd() -> Herd {
        let mut h = Herd::new();
        h.pets.push(Pet::new(
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

    #[test]
    fn resize_purges_and_retransmits() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_working_herd();
        let _ = r.render_pets(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, 0);
        let _ = sink.take();
        // Same area: image stays cached, no re-transmit.
        let _ = r.render_pets(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, 0);
        assert!(
            !sink.take().contains("a=t"),
            "unchanged area reuses the cache"
        );
        // Changed area (resize): purge everything and re-transmit fresh.
        let _ = r.render_pets(&herd, &species, Rect::new(0, 0, 120, 8), Theme::Dark, 0);
        let out = sink.take();
        assert!(out.contains("a=d,d=A"), "resize deletes all prior images");
        assert!(out.contains("a=t"), "resize re-transmits the image");
    }

    #[test]
    fn teardown_deletes_all_images() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        r.teardown().unwrap();
        assert_eq!(sink.take(), "\x1b_Ga=d,d=A\x1b\\");
    }

    #[test]
    fn pet_size_stays_small_and_keeps_aspect() {
        // Rows are capped small even in a very tall pane, and never below 2.
        assert_eq!(pet_rows(3), 2);
        assert_eq!(pet_rows(6), 4);
        assert_eq!(pet_rows(40), 4);
        // The caption moved into the top lane, freeing the row it used to
        // reserve: a 4-row pane now gets 3 pet rows, not 2.
        assert_eq!(
            pet_rows(4),
            3,
            "reclaims the row previously reserved for the bottom caption"
        );
        // Cols preserve the sprite aspect (16x14 -> ~2.3 cols per row).
        assert_eq!(pet_cols(3, 16, 14), 7); // round(3 * 16/14 * 2.1)
        assert_eq!(pet_cols(4, 16, 14), 10);
    }

    #[test]
    fn draw_places_the_hover_caption_top_right_via_the_ratatui_frame() {
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_working_herd();
        let mut r = KittyRenderer::for_test(SharedSink::default(), 4);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let completed = terminal
            .draw(|f| {
                PetRenderer::draw(&mut r, f, &herd, &species, Theme::Dark, 0, Some("agent-x"))
            })
            .unwrap();
        let row0: String = (0..completed.buffer.area.width)
            .map(|x| {
                completed.buffer[(x, 0)]
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(
            row0.contains("agent-x"),
            "caption drawn top-right via the ratatui frame: {row0:?}"
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
        r.draw_to_sink(&herd, &species, Theme::Dark, 0); // populates last_area for pet_rows
        let hit = (0..200u16).find_map(|c| r.pet_at_column(&herd, &species, 200, c, 0));
        assert_eq!(hit, Some(0), "some column under the pet hits it");
        assert_eq!(
            r.pet_at_column(&herd, &species, 200, 200, 0),
            None,
            "column past the strip's edge is empty"
        );
    }

    fn one_idle_herd() -> Herd {
        let mut h = Herd::new();
        h.pets.push(Pet::new(
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
        let scale = 7; // production default (`Config::default().pet_scale`)
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
        // Exactly one transmit (the pet's own padded image), no second icon image.
        assert_eq!(out.matches("a=t").count(), 1, "working carries no icon");
    }

    #[test]
    fn idle_transmits_and_places_both_the_pet_and_its_zz_icon() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        r.draw_to_sink(&one_idle_herd(), &species, Theme::Dark, 0);
        let out = sink.take();
        assert_eq!(
            out.matches("a=t").count(),
            2,
            "the pet image and the Zz icon image"
        );
        assert_eq!(
            out.matches("a=p").count(),
            2,
            "the pet placement and the icon placement"
        );
        // Placements are cropped-source (x=/y=/w=/h=), not the old fixed-size form.
        assert!(out.contains("x=") && out.contains("y="));
    }

    #[test]
    fn losing_the_overlay_deletes_the_stale_icon_placement() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let _ = r.render_pets(
            &one_idle_herd(),
            &species,
            Rect::new(0, 0, 200, 10),
            Theme::Dark,
            0,
        );
        let _ = sink.take();
        // Same pet, now working (no overlay): the old Zz icon placement must
        // be torn down, not left as a ghost badge.
        let _ = r.render_pets(
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
            "only the new working pet frame is transmitted, no icon"
        );
    }

    #[test]
    fn blocked_pet_placement_pans_as_time_advances() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        herd.pets.push(Pet::new(
            "t1".into(),
            identity_for("t1", 1),
            AgentStatus::Blocked,
        ));

        // Isolate the PET's placement command (z=0; the icon badge places at
        // z=1000+ and would otherwise pollute the comparison).
        let pet_placement = |out: &str| -> String {
            out.split("\x1b_G")
                .find(|chunk| chunk.contains("a=p") && chunk.contains(",z=0,"))
                .expect("the pet's own placement command")
                .to_string()
        };
        let y_field = |chunk: &str| -> String {
            chunk
                .split(',')
                .find(|p| p.starts_with("y="))
                .expect("a y= field")
                .to_string()
        };

        let _ = r.render_pets(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, 0);
        let y0 = y_field(&pet_placement(&sink.take()));

        // Some particular pair of instants could coincidentally round to the
        // same pixel; scan a spread of them so the test isn't tied to one
        // sample landing on a flat spot in the bounce curve.
        let panned = (10..500u64).step_by(10).any(|ms| {
            let _ = r.render_pets(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, ms);
            y_field(&pet_placement(&sink.take())) != y0
        });
        assert!(
            panned,
            "bounce motion must pan the crop window as time advances"
        );
    }

    fn one_focused_working_herd() -> Herd {
        let mut h = Herd::new();
        let mut pet = Pet::new("t1".into(), identity_for("t1", 1), AgentStatus::Working);
        pet.focused = true;
        h.pets.push(pet);
        h
    }

    #[test]
    fn focused_pet_emits_a_hat_image_and_placement() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        r.draw_to_sink(&one_focused_working_herd(), &species, Theme::Dark, 0);
        let out = sink.take();
        assert_eq!(
            out.matches("a=t").count(),
            2,
            "the pet's own frame and the hat image are both transmitted"
        );
        assert_eq!(
            out.matches("a=p").count(),
            2,
            "the pet placement and the hat placement"
        );
    }

    #[test]
    fn unfocused_pet_emits_no_hat() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        r.draw_to_sink(&one_working_herd(), &species, Theme::Dark, 0);
        let out = sink.take();
        assert_eq!(
            out.matches("a=t").count(),
            1,
            "only the pet's own frame; no hat image"
        );
        assert_eq!(
            out.matches("a=p").count(),
            1,
            "only the pet placement; no hat placement"
        );
    }

    #[test]
    fn hat_placement_is_torn_down_when_focus_moves_away() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let _ = r.render_pets(
            &one_focused_working_herd(),
            &species,
            Rect::new(0, 0, 200, 10),
            Theme::Dark,
            0,
        );
        let _ = sink.take();
        // Same pet, now unfocused: the old hat placement must be torn down,
        // and neither the (cached) body nor the (cached) hat is re-transmitted.
        let _ = r.render_pets(
            &one_working_herd(),
            &species,
            Rect::new(0, 0, 200, 10),
            Theme::Dark,
            0,
        );
        let out = sink.take();
        // Two deletes: the body's own old placement (redrawn every frame
        // regardless of focus) AND the now-stale hat placement.
        assert_eq!(
            out.matches("a=d").count(),
            2,
            "the body's old placement and the stale hat placement are both deleted: {out:?}"
        );
        assert_eq!(
            out.matches("a=t").count(),
            0,
            "body and hat images are both already cached — no re-transmit"
        );
    }

    #[test]
    fn hat_placement_is_torn_down_when_the_focused_pet_departs() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let _ = r.render_pets(
            &one_focused_working_herd(),
            &species,
            Rect::new(0, 0, 200, 10),
            Theme::Dark,
            0,
        );
        let _ = sink.take();
        // The pet is gone entirely (empty herd): its hat placement must not
        // linger as a ghost image.
        let _ = r.render_pets(
            &Herd::new(),
            &species,
            Rect::new(0, 0, 200, 10),
            Theme::Dark,
            0,
        );
        let out = sink.take();
        // Two deletes: the departed pet's body placement AND its hat placement.
        assert_eq!(
            out.matches("a=d").count(),
            2,
            "the departed pet's body and hat placements are both deleted: {out:?}"
        );
    }

    #[test]
    fn hat_placement_pans_as_time_advances() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_focused_working_herd();

        // Isolate the HAT's placement command by its distinctive high z index
        // (Z_HAT_BASE, far above the body's z=0 and the icon's Z_ICON_BASE+).
        let hat_placement = |out: &str| -> String {
            out.split("\x1b_G")
                .find(|chunk| chunk.contains("a=p") && chunk.contains(",z=100000,"))
                .expect("the hat's own placement command")
                .to_string()
        };
        let y_field = |chunk: &str| -> String {
            chunk
                .split(',')
                .find(|p| p.starts_with("y="))
                .expect("a y= field")
                .to_string()
        };

        let _ = r.render_pets(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, 0);
        let y0 = y_field(&hat_placement(&sink.take()));

        let panned = (10..500u64).step_by(10).any(|ms| {
            let _ = r.render_pets(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark, ms);
            y_field(&hat_placement(&sink.take())) != y0
        });
        assert!(
            panned,
            "the hat must pan with the pet's own motion offset, exactly like the body"
        );
    }

    #[test]
    fn focused_idle_pet_emits_both_its_zz_icon_and_its_hat() {
        // A focused, idle pet shows both overlays at once — the hat must not
        // replace or be replaced by the icon.
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        let mut pet = Pet::new("t1".into(), identity_for("t1", 1), AgentStatus::Idle);
        pet.focused = true;
        herd.pets.push(pet);
        r.draw_to_sink(&herd, &species, Theme::Dark, 0);
        let out = sink.take();
        assert_eq!(
            out.matches("a=t").count(),
            3,
            "the pet frame, the Zz icon, and the hat"
        );
        assert_eq!(
            out.matches("a=p").count(),
            3,
            "the pet placement, the icon placement, and the hat placement"
        );
    }
}
