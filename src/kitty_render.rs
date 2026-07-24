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
use crate::herd::{Herd, visible_and_hidden};
use crate::kitty::{delete_all, delete_placement, place_sized, transmit_rgba};
use crate::palette::{StateStyle, Theme, role_color};
use crate::pet::priority;
use crate::raster::rasterize;
use crate::render::PetRenderer;
use crate::sprite::{Frame as SpriteFrame, Species};

/// Rows a pet image occupies, derived from the pane height (reserving the
/// caption row plus a little headroom) and capped small so the pets always
/// read as a slim strip, even in a tall pane.
fn pet_rows(pane_h: u16) -> u16 {
    pane_h.saturating_sub(2).clamp(2, 4)
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
    ) -> io::Result<()> {
        // On a geometry change (resize), the terminal may have dropped our
        // transmitted images. Purge everything and re-transmit fresh this
        // frame, or `place` would reference gone images and leave the strip
        // blank. The per-pet positions below already reflow to the new area.
        if self.last_area != Some(area) {
            self.out.write_all(delete_all().as_bytes())?;
            self.cache.clear();
            self.placements.clear();
            self.last_area = Some(area);
        }

        let strip_w = area.width as usize;
        let pet_w = species.first().map(|s| s.size().0).unwrap_or(12);
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
            let fi = pet.frame_index(state.frames.len());
            let fr = &state.frames[fi];
            let key: ImgKey = (
                pet.identity.species_index,
                pet.status,
                fi,
                pet.facing_left,
                pet.identity.hue,
            );
            let image_id = match self.cache.get(&key) {
                Some(&id) => id,
                None => {
                    let style = StateStyle {
                        dim: state.dim,
                        ghost: state.ghost,
                    };
                    let rgba = rasterize(
                        fr,
                        pet.identity.hue,
                        theme,
                        style,
                        self.scale,
                        pet.facing_left,
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
            let col = (pet.x.round() as i32 + 1).clamp(1, area.width.max(1) as i32);
            self.out
                .write_all(format!("\x1b[{row};{col}H").as_bytes())?;

            let pid = self.next_id;
            self.next_id += 1;
            // z = draw-order index: later-drawn (higher priority) stacks on top,
            // so kitty's visual stacking matches the hit-test's last-wins order.
            self.out
                .write_all(place_sized(image_id, pid, cols, rows, zi as i32).as_bytes())?;

            if let Some((old_img, old_pid)) = self
                .placements
                .insert(pet.terminal_id.clone(), (image_id, pid))
            {
                // Draw-then-delete: the new placement is already on screen
                // before the old one disappears, so there is no blank frame.
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

        Ok(())
    }

    /// Test-only entry point that skips the ratatui `Frame` entirely and
    /// drives `render_pets` with a fixed `strip_w` matching the hit-test
    /// tests (200 columns). Errors are swallowed, mirroring `draw`'s
    /// tolerance for a failed frame.
    #[cfg(test)]
    pub fn draw_to_sink(&mut self, herd: &Herd, species: &[Species], theme: Theme) {
        let _ = self.render_pets(herd, species, Rect::new(0, 0, 200, 10), theme);
    }
}

impl PetRenderer for KittyRenderer {
    /// Kitty images are drawn out of band (direct terminal escapes), not into
    /// the ratatui buffer — `frame` is only consulted for its width. A failed
    /// write degrades to a skipped frame rather than crashing the strip (the
    /// render loop already tolerates this for the half-block path).
    fn draw(&mut self, frame: &mut Frame, herd: &Herd, species: &[Species], theme: Theme) {
        let _ = self.render_pets(herd, species, frame.area(), theme);
    }

    /// Hit-test using the same visible set as `render_pets`. A pet's hit range
    /// is the on-screen span of its sprite's *opaque* pixels (transparent frame
    /// padding is excluded, so hover matches the visible sheep), converted from
    /// pixels to cells. We iterate in the SAME z-order `render_pets` draws
    /// (priority-sorted, stable) and let the LAST covering pet win — the one
    /// drawn on top — so hover selects the sprite that is visually in front when
    /// pets overlap, instead of one hidden behind it.
    fn pet_at_column(
        &self,
        herd: &Herd,
        species: &[Species],
        strip_w: usize,
        col: u16,
    ) -> Option<usize> {
        let base_w = species.first().map(|s| s.size().0).unwrap_or(12);
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
            let fi = pet.frame_index(state.frames.len());
            let fr = &state.frames[fi];
            // The image occupies `cols` cells from `pet.x`; the visible sprite
            // is the opaque span scaled into those cols, so hover matches the
            // sheep, not its transparent padding.
            let cols = pet_cols(rows, fr.w, fr.h) as usize;
            let (lo, hi) = opaque_col_span(fr, pet.facing_left).unwrap_or((0, fr.w));
            // Round each opaque edge to the nearest cell (rather than
            // floor-left/ceil-right) so the hit region hugs the visible sprite
            // instead of over-reaching into a barely-touched edge cell.
            let left_cell = (lo * cols + fr.w / 2) / fr.w;
            let right_cell = ((hi * cols + fr.w / 2) / fr.w).max(left_cell + 1);
            let left = pet.x.round() as i32;
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
            4.0,
        ));
        h
    }

    #[test]
    fn draw_transmits_then_places_and_second_frame_reuses_the_image() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4);
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_working_herd();
        r.draw_to_sink(&herd, &species, Theme::Dark); // test-only wrapper (no ratatui)
        let first = sink.take();
        assert!(first.contains("a=t"), "first draw transmits the image");
        assert!(first.contains("a=p"), "and places it");
        r.draw_to_sink(&herd, &species, Theme::Dark);
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
        let _ = r.render_pets(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark);
        let _ = sink.take();
        // Same area: image stays cached, no re-transmit.
        let _ = r.render_pets(&herd, &species, Rect::new(0, 0, 200, 10), Theme::Dark);
        assert!(
            !sink.take().contains("a=t"),
            "unchanged area reuses the cache"
        );
        // Changed area (resize): purge everything and re-transmit fresh.
        let _ = r.render_pets(&herd, &species, Rect::new(0, 0, 120, 8), Theme::Dark);
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
        // Cols preserve the sprite aspect (16x14 -> ~2.3 cols per row).
        assert_eq!(pet_cols(3, 16, 14), 7); // round(3 * 16/14 * 2.1)
        assert_eq!(pet_cols(4, 16, 14), 10);
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
        let r = KittyRenderer::for_test(SharedSink::default(), 4);
        // pet at x=4, footprint = ceil(frame_w*scale / cell_w) columns wide.
        assert_eq!(r.pet_at_column(&herd, &species, 200, 4), Some(0));
        assert_eq!(r.pet_at_column(&herd, &species, 200, 190), None);
    }
}
