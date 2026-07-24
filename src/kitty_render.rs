//! Kitty graphics protocol backend for [`PetRenderer`]: transmits each
//! distinct sprite frame once and caches the image id, places/re-places it at
//! the pet's cell every frame, and deletes placements for pets that departed
//! or fell out of the visible set. Escapes are written to an injected
//! `io::Write` sink (real stdout in production, a `Vec<u8>`-backed sink in
//! tests) so the encoding is unit-testable without a real terminal.

use std::collections::HashMap;
use std::io::{self, Write};

use ratatui::Frame;

use crate::agent::AgentStatus;
use crate::herd::{Herd, visible_and_hidden};
use crate::kitty::{delete_all, delete_placement, place, transmit_rgba};
use crate::palette::{StateStyle, Theme};
use crate::pet::priority;
use crate::raster::rasterize;
use crate::render::PetRenderer;
use crate::sprite::Species;

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
    cell_px: (u16, u16),
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
}

impl KittyRenderer {
    /// Build a renderer that writes to `out` (stdout in production) at sprite
    /// `scale` with terminal cell size `cell_px` in pixels (from a `CSI 14 t`
    /// query, or the `(8, 16)` fallback — both resolved by the caller; this
    /// task does not query).
    pub fn new(scale: usize, cell_px: (u16, u16), out: Box<dyn Write + Send>) -> Self {
        Self {
            scale,
            cell_px,
            out,
            cache: HashMap::new(),
            placements: HashMap::new(),
            next_id: 1,
        }
    }

    /// Convenience constructor for production wiring: writes straight to
    /// stdout with a conservative default cell size. Real cell-size detection
    /// (`CSI 14 t`) is wired by the caller, not here.
    pub fn new_stdout(scale: usize) -> Self {
        Self::new(scale, (8, 16), Box::new(io::stdout()))
    }

    /// Write all escapes for the current frame's visible pets to `self.out`,
    /// replicating `render::draw_herd`'s visible-set + z-order selection
    /// exactly so the kitty and half-block backends agree on what's shown.
    fn render_pets(
        &mut self,
        herd: &Herd,
        species: &[Species],
        strip_w: usize,
        theme: Theme,
    ) -> io::Result<()> {
        let pet_w = species.first().map(|s| s.size().0).unwrap_or(12);
        let capacity = (strip_w / (pet_w * 3 / 4).max(1)).max(1);
        let (visible, _hidden) = visible_and_hidden(&herd.pets, capacity);

        let mut order = visible.clone();
        order.sort_by_key(|&i| priority(herd.pets[i].status));

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

            let col = pet.x.round() as i32 + 1; // 1-based CSI cursor coords
            let row = 1; // band is bottom-anchored by the caller's cursor position
            self.out
                .write_all(format!("\x1b[{row};{col}H").as_bytes())?;

            let pid = self.next_id;
            self.next_id += 1;
            self.out.write_all(place(image_id, pid).as_bytes())?;

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
        let _ = self.render_pets(herd, species, 200, theme);
    }
}

impl PetRenderer for KittyRenderer {
    /// Kitty images are drawn out of band (direct terminal escapes), not into
    /// the ratatui buffer — `frame` is only consulted for its width. A failed
    /// write degrades to a skipped frame rather than crashing the strip (the
    /// render loop already tolerates this for the half-block path).
    fn draw(&mut self, frame: &mut Frame, herd: &Herd, species: &[Species], theme: Theme) {
        let strip_w = frame.area().width as usize;
        let _ = self.render_pets(herd, species, strip_w, theme);
    }

    /// Hit-test using the same visible set as `render_pets`; a pet's cell
    /// footprint is its rasterized width (in pixels) divided by the terminal
    /// cell width, rounded up.
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

        let x = col as i32;
        let mut best: Option<usize> = None;
        for &i in &visible {
            let pet = &herd.pets[i];
            let frame_w = species
                .get(pet.identity.species_index)
                .or_else(|| species.first())
                .map(|s| s.size().0)
                .unwrap_or(base_w);
            let footprint_px = frame_w * self.scale;
            let footprint_cells = footprint_px.div_ceil(self.cell_px.0 as usize).max(1) as i32;
            let left = pet.x.round() as i32;
            if x >= left && x < left + footprint_cells {
                let take = match best {
                    None => true,
                    Some(b) => priority(pet.status) >= priority(herd.pets[b].status),
                };
                if take {
                    best = Some(i);
                }
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
    pub fn for_test(sink: SharedSink, scale: usize, cell_px: (u16, u16)) -> Self {
        Self::new(scale, cell_px, Box::new(sink))
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
        let mut r = KittyRenderer::for_test(sink.clone(), 4, (8, 16));
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
    fn teardown_deletes_all_images() {
        let sink = SharedSink::default();
        let mut r = KittyRenderer::for_test(sink.clone(), 4, (8, 16));
        r.teardown().unwrap();
        assert_eq!(sink.take(), "\x1b_Ga=d,d=A\x1b\\");
    }

    #[test]
    fn hit_test_uses_the_cell_footprint() {
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = one_working_herd();
        let r = KittyRenderer::for_test(SharedSink::default(), 4, (8, 16));
        // pet at x=4, footprint = ceil(frame_w*scale / cell_w) columns wide.
        assert_eq!(r.pet_at_column(&herd, &species, 200, 4), Some(0));
        assert_eq!(r.pet_at_column(&herd, &species, 200, 190), None);
    }
}
