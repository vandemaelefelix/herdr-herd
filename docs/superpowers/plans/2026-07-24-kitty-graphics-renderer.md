# Kitty-graphics Rendering Backend — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Also required per repo CLAUDE.md:** before writing Rust in any task, invoke the relevant project skill (`rust-error-handling`, `rust-testability-seams`, `rust-serde-tolerant-parsing`, `rust-tui-snapshot-testing`, `rust-project-conventions`) so new code matches house style.

**Goal:** Render the pets as small, crisp, full-detail, animated kitty-graphics images (with a universal half-block fallback), restore the 16×14 artifact sprites, and make pets stay still when idle / amble slowly and face their travel direction when working.

**Architecture:** The pet *simulation* (herd roam, position, frame selection, z-order) stays rendering-agnostic. A new `PetRenderer` trait has two implementations — `HalfBlockRenderer` (today's `▀▄` path) and `KittyRenderer` (kitty graphics). One is chosen at startup by config + a self-correcting capability probe.

**Tech Stack:** Rust (edition per repo), ratatui + crossterm (already used), kitty graphics protocol (escape sequences we emit ourselves). No new runtime crates.

## Global Constraints

- **No new runtime crate dependencies.** Base64 and RGBA rasterization are hand-rolled. (`toml`/`insta` stay dev-only.) — verbatim from spec §2.
- **Green gate must pass before any task is "done":** `cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check`.
- **Error handling:** `Result` + `?`, `io::Error::other`, no `unwrap`/`expect` outside tests (per `rust-error-handling`).
- **Testability:** anything touching the terminal/tty/clock goes behind a trait with Real/Fake impls (per `rust-testability-seams`); tests never touch a real terminal.
- **Commits:** Conventional Commits, imperative, lower-case, no trailing period. Scope by module (`render`, `sprite`, `config`, `herd`). One commit per task step-5.
- **Branching:** work continues on the stack tip `feat/pets-every-tab` (do not branch off `main`; see `docs/decisions.md` "Stacked phase branches"). Do **not** commit/push without the maintainer's go-ahead — propose commits.
- **GOAL.md invariants:** universal-first (half-block always works); kitty is an opt-in upgrade; stable identity; click-to-focus + hover-label preserved; unobtrusive.

---

# STEP A — Restore artifact sprites + shared behavior (universal, half-block)

Ships a coherent improvement on every terminal even if Step B slips.

---

### Task A1: Restore the 16×14 artifact sprites and un-slim the half-block band

**Files:**
- Modify: `sprites/sheep.sprite` (replace body with v2 art)
- Modify: `sprites/goat.sprite` (replace body with v2 art)
- Modify: `src/render.rs` (`PET_PX_H`)
- Modify: `src/sprite.rs` (the `h <= 5` guard in `every_embedded_species_is_valid`)
- Modify: `src/anim.rs` (hop/shake caps if they assume a 1px headroom)
- Test: `src/sprite.rs` (existing guard test), `src/render.rs` (snapshot tests)

**Interfaces:**
- Produces: species size `(16, 14)`; `PET_PX_H = 14`; sprites with `idle/working/done/blocked/unknown` states, `working` = 2 walk frames.

- [ ] **Step 1: Replace `sprites/sheep.sprite` with the v2 artifact art.** Recover verbatim with `git show 2dc1378:sprites/sheep.sprite > sprites/sheep.sprite`. It must match:

```
name = Sheep

# Traced from the maintainer's sprite-playground artifact (sheep_assets_x4):
# a side-view sheep. idle = the lying-down "dozing" pose (row4_f0); working =
# the two-frame walk cycle (row1_f0/f1); done/blocked/unknown = the standing
# pose. All frames are normalised to 16x14 (shorter poses padded on top, so the
# feet stay on the ground).

[idle]    frame_ms=520 motion=breathe overlay=bubble:Zz
................
................
................
................
................
.......####.....
...####MMMM#....
..#SSSMMMMM#....
.#MMMMSMSSS#....
#MMMMMMSMMMM#...
#MMSSSSMMMMM#...
.#SSMMMSMMM#....
..#MMMMM###.....
...#####........

[working] frame_ms=150 motion=hop+wander overlay=none
................
...........###..
........###MMM#.
...#####SpMSMSS#
..#MMMMM##Mp#p#.
.#SSMSMSMSMpepp#
#SMMMMMMMMMpppp#
.#MMMMSMSMSMS##.
.#MSMMMMMMMMM#..
..#MMMMSMMMMS#..
..#MSMSSSMMS#...
..#SS###SSS#....
..#MM#..#MM#....
...##....##.....

................
...........###..
........###MMM#.
...#####SpMSMSS#
..#MMMMM##Mp#p#.
.#SSMSMSMSMpepp#
#SMMMMMMMMMpppp#
.#MMMMSMSMSMS##.
.#MSMMMMMMMMM#..
..#MMMMSMMMMS#..
..#MSMSSSMMS#...
.#MMS###MMS#....
.#MM#..#MS#.....
..##....##......

[done]    frame_ms=1400 motion=hop overlay=badge:! color=accent
................
...........###..
........###MMM#.
...#####SpMSMSS#
..#MMMMM##Mp#p#.
.#SSMSMSMSMpepp#
#SMMMMMMMMMpppp#
.#MMMMSMSMSMS##.
.#MSMMMMMMMMM#..
..#MMMMSMMMMS#..
..#MSMSSSMMS#...
..#SS###SSS#....
..#MM#..#MM#....
...##....##.....

[blocked] frame_ms=120 motion=shake overlay=badge:! color=#e62d23
................
...........###..
........###MMM#.
...#####SpMSMSS#
..#MMMMM##Mp#p#.
.#SSMSMSMSMpepp#
#SMMMMMMMMMpppp#
.#MMMMSMSMSMS##.
.#MSMMMMMMMMM#..
..#MMMMSMMMMS#..
..#MSMSSSMMS#...
..#SS###SSS#....
..#MM#..#MM#....
...##....##.....

................
...........###..
........###MMM#.
...#####SpMSMSS#
..#MMMMM##Mp#p#.
.#SSMSMSMSMpepp#
#SMMMMMMMMMpppp#
.#MMMMSMSMSMS##.
.#MSMMMMMMMMM#..
..#MMMMSMMMMS#..
..#MSMSSSMMS#...
.#MMS###MMS#....
.#MM#..#MS#.....
..##....##......

[unknown] frame_ms=0 motion=sway overlay=bubble:? ghost=true
................
...........###..
........###MMM#.
...#####SpMSMSS#
..#MMMMM##Mp#p#.
.#SSMSMSMSMpepp#
#SMMMMMMMMMpppp#
.#MMMMSMSMSMS##.
.#MSMMMMMMMMM#..
..#MMMMSMMMMS#..
..#MSMSSSMMS#...
..#SS###SSS#....
..#MM#..#MM#....
...##....##.....
```

- [ ] **Step 2: Replace `sprites/goat.sprite` with the v2 art.** `git show 2dc1378:sprites/goat.sprite > sprites/goat.sprite` (same poses + a horn `h`; 16×14). Verify it starts `name = Goat` and each state block is 14 rows × 16 cols (the idle block has the horn `hh` on row 5; the standing blocks have `hh`/`h` on the top two rows).

- [ ] **Step 3: Update the half-block band sizing.** In `src/render.rs`, change the band-height constant so the full sprite fits:

```rust
/// Height of the pet band in pixels. Sprites are 16x14 (see sprites/*.sprite);
/// the band is the sprite height plus 1px of headroom for the hop/shake lift.
pub const PET_PX_H: usize = 15;
```

Check `draw_herd`'s `band_rows = PET_PX_H.div_ceil(2)` and the bottom-anchor math still hold (they are expressed in terms of `PET_PX_H`, so they scale automatically). The bottom-align + `floor = PET_PX_H - fr.h` logic is unchanged.

- [ ] **Step 4: Relax the sprite height guard.** In `src/sprite.rs` `every_embedded_species_is_valid`, change the assertion from `h <= 5` to match the new band (sprite ≤ `PET_PX_H - 1`):

```rust
assert!(
    h <= 14,
    "{} must be <= 14 px (1 px shorter than the 15 px band) so \
     the hop/shake lift never clips",
    sp.name
);
```

- [ ] **Step 5: Re-cap the hop/shake if needed.** Review `src/anim.rs` `motion_offset`: `Hop`/`Shake` lift is ≤1px, which fits the 1px headroom — no change required. Add a code comment noting the headroom is now 1px of a 15px band. (If a larger, livelier hop is desired later, that's a separate change; keep ≤1px here so nothing clips.)

- [ ] **Step 6: Run the guard + parse tests.**

Run: `cargo test -p herdr-pets sprite`
Expected: PASS (`every_embedded_species_is_valid`, `parses_name_states_and_frame_grid`, etc.). If `every_embedded_species_is_valid` fails on size, the sprite files are ragged — fix the offending file.

- [ ] **Step 7: Refresh the render snapshots.**

Run: `cargo test -p herdr-pets render` — snapshot tests (`renders_each_state_in_the_strip`, `renders_overflow_counter`, `caption_shows_the_hovered_name_on_the_bottom_row`, etc.) will fail because the art + band height changed.
Then review each changed snapshot with `cargo insta review` (accept only after eyeballing that the strip shows the detailed sheep bottom-anchored, no clipping). Commit the updated `.snap` files.
Expected after accept: PASS.

- [ ] **Step 8: Full gate + commit.**

```bash
cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check
git add sprites/sheep.sprite sprites/goat.sprite src/render.rs src/sprite.rs src/anim.rs src/snapshots/
git commit -m "feat(render): restore the 16x14 artifact sprites at full detail"
```

---

### Task A2: Idle pets stand still; only working pets amble (slowly)

**Files:**
- Modify: `src/herd.rs` (`step`)
- Test: `src/herd.rs` (tests module)

**Interfaces:**
- Consumes: `Pet.status`, `Pet.x`, `Pet.target_x`.
- Produces: after `step`, a non-`Working` pet's `x` is unchanged from its pre-step value; `Working` pets move slowly toward `target_x`.

- [ ] **Step 1: Write the failing test** (add to `src/herd.rs` tests):

```rust
#[test]
fn only_working_pets_roam_horizontally() {
    let mut h = Herd::new();
    let mut rng = Lcg::new(11);
    h.reconcile(
        &[
            agent("idle", AgentStatus::Idle),
            agent("done", AgentStatus::Done),
            agent("blk", AgentStatus::Blocked),
        ],
        1,
        200.0,
        &mut rng,
    );
    let before: Vec<f32> = h.pets.iter().map(|p| p.x).collect();
    for _ in 0..200 {
        h.step(50.0, 200.0, 16.0, &mut rng);
    }
    for (p, x0) in h.pets.iter().zip(before) {
        assert_eq!(p.x, x0, "{} must not roam when not working", p.terminal_id);
    }
}
```

- [ ] **Step 2: Run it to see it fail.**

Run: `cargo test -p herdr-pets only_working_pets_roam`
Expected: FAIL (idle/done drift today at roam 0.35).

- [ ] **Step 3: Make only working pets roam, slowly.** In `src/herd.rs` `step`, replace the `roam`/`speed` selection:

```rust
// Only working pets roam horizontally; everyone else holds position
// (they still animate in place via motion_offset). The amble is a gentle
// "I'm working" signal — slow enough to stay easily clickable.
let (roam, speed) = match p.status {
    crate::agent::AgentStatus::Working => (1.0_f32, 9.0_f32),
    _ => (0.0_f32, 0.0_f32),
};
if roam > 0.0 && rng.next_unit() < roam * dt * 0.4 {
    p.target_x = rng.next_unit() * max_x;
}
let dx = p.target_x - p.x;
p.x += dx.signum() * dx.abs().min(speed * dt);
```

(Speed `9.0` px/s and target-change rate `0.4` reproduce the approved slow amble; tune with the maintainer if needed. Non-working pets get `roam=0`, so their `target_x` is never repicked and `x` never changes.)

- [ ] **Step 4: Run the test + the existing bounds test.**

Run: `cargo test -p herdr-pets herd`
Expected: PASS (`only_working_pets_roam_horizontally`, `step_keeps_pets_within_bounds`, `reconcile_*`). If `step_keeps_pets_within_bounds` uses all-`Working` agents it still moves and stays clamped — verify it still passes; it should.

- [ ] **Step 5: Gate + commit.**

```bash
cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check
git add src/herd.rs
git commit -m "feat(herd): only working pets roam; idle pets hold still"
```

---

### Task A3: Pets face their direction of travel

**Files:**
- Modify: `src/pet.rs` (add `facing` field + update on move)
- Modify: `src/herd.rs` (`step` sets facing from movement)
- Modify: `src/render.rs` (`draw_herd` mirrors the frame when facing left)
- Test: `src/pet.rs`, `src/herd.rs`, `src/render.rs`

**Interfaces:**
- Produces: `Pet.facing_left: bool` (default `false` = faces right, matching the sprite art). `Pet::set_facing_from_dx(dx: f32)` updates it, keeping the last non-zero direction.
- Consumes (render): when `facing_left`, blit the frame mirrored on x (`col = fr.w - 1 - x`).

- [ ] **Step 1: Write the failing `Pet` test** (`src/pet.rs` tests):

```rust
#[test]
fn facing_tracks_last_nonzero_direction() {
    let mut p = pet(AgentStatus::Working);
    assert!(!p.facing_left, "defaults to facing right (sprite art faces right)");
    p.set_facing_from_dx(-2.0);
    assert!(p.facing_left, "moving left faces left");
    p.set_facing_from_dx(0.0);
    assert!(p.facing_left, "no movement keeps the last facing");
    p.set_facing_from_dx(3.0);
    assert!(!p.facing_left, "moving right faces right");
}
```

- [ ] **Step 2: Run it to see it fail.**

Run: `cargo test -p herdr-pets facing_tracks`
Expected: FAIL (`facing_left`/`set_facing_from_dx` don't exist).

- [ ] **Step 3: Add the field + method** to `src/pet.rs`. Add `pub facing_left: bool` to `Pet`, initialize `false` in `Pet::new`, and:

```rust
/// Update facing from a horizontal delta; zero delta keeps the last facing
/// so a pet that stops does not snap back to a default direction.
pub fn set_facing_from_dx(&mut self, dx: f32) {
    if dx > 0.0 {
        self.facing_left = false;
    } else if dx < 0.0 {
        self.facing_left = true;
    }
}
```

- [ ] **Step 4: Run the `Pet` test.** Run: `cargo test -p herdr-pets facing_tracks` → PASS.

- [ ] **Step 5: Set facing in the herd step.** In `src/herd.rs` `step`, after computing the applied delta for a pet, record facing. Change the move line to capture the delta:

```rust
let dx = p.target_x - p.x;
let applied = dx.signum() * dx.abs().min(speed * dt);
p.x += applied;
p.set_facing_from_dx(applied);
```

- [ ] **Step 6: Write the failing render mirror test** (`src/render.rs` tests). A left-facing working pet must place head pixels on the left; simplest deterministic check — mirror is applied. Add:

```rust
#[test]
fn left_facing_pet_is_mirrored() {
    // Build a herd with one working pet, force facing_left, freeze it, and
    // assert the rendered band differs from the same pet facing right.
    use crate::agent::AgentStatus::*;
    let species = vec![parse_species(BLOB).unwrap()];
    let mut right = fixed_herd(&[Working]);
    right.pets[0].facing_left = false;
    let mut left = fixed_herd(&[Working]);
    left.pets[0].facing_left = true;
    let render = |h: &Herd| {
        let mut t = Terminal::new(TestBackend::new(40, 8)).unwrap();
        t.draw(|f| draw_herd(f, h, &species, Theme::Dark)).unwrap();
        format!("{}", t.backend())
    };
    assert_ne!(render(&right), render(&left), "mirroring must change the pixels");
}
```

(`BLOB` in the test fixture must have an asymmetric frame so mirroring changes output; `test-blob.sprite` — verify/adjust the fixture so at least one working frame is left-right asymmetric.)

- [ ] **Step 7: Run it to see it fail.** Run: `cargo test -p herdr-pets left_facing_pet_is_mirrored` → FAIL (mirror not implemented).

- [ ] **Step 8: Implement mirroring in `draw_herd`.** In the pixel blit loop, compute the source column with a flip when `pet.facing_left`:

```rust
for y in 0..fr.h {
    for x in 0..fr.w {
        let sx = if pet.facing_left { fr.w - 1 - x } else { x };
        if let Some(c) = role_color(fr.cells[y * fr.w + sx], pet.identity.hue, theme, style) {
            buf.set(ox + x as i32, oy + y as i32, c);
        }
    }
}
```

- [ ] **Step 9: Run mirror test + refresh other snapshots.** Run: `cargo test -p herdr-pets render`; `cargo insta review` for any legitimately-changed snapshots (facing defaults to right, so most are unchanged). → PASS.

- [ ] **Step 10: Gate + commit.**

```bash
cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check
git add src/pet.rs src/herd.rs src/render.rs src/snapshots/
git commit -m "feat(render): pets face their direction of travel"
```

---

# STEP B — Kitty-graphics rendering backend

New pure modules first (fully unit-testable), then the trait seam, then wiring.

---

### Task B1: Hand-rolled base64 encoder

**Files:**
- Create: `src/base64.rs`
- Modify: `src/lib.rs` (add `pub mod base64;`)
- Test: `src/base64.rs`

**Interfaces:**
- Produces: `pub fn encode(bytes: &[u8]) -> String` — standard base64 (`+`/`/`, `=` padding).

- [ ] **Step 1: Write failing tests** (`src/base64.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::encode;
    #[test]
    fn encodes_rfc4648_vectors() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }
}
```

- [ ] **Step 2: Run to see it fail.** Run: `cargo test -p herdr-pets base64` → FAIL (module empty).

- [ ] **Step 3: Implement.**

```rust
//! Minimal standard base64 encoder (RFC 4648). Hand-rolled to avoid a runtime
//! dependency; used only to encode kitty-graphics image payloads.

const ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode `bytes` as standard base64 with `=` padding.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[(n & 63) as usize] as char } else { '=' });
    }
    out
}
```

- [ ] **Step 4: Run to see it pass.** Run: `cargo test -p herdr-pets base64` → PASS.

- [ ] **Step 5: Gate + commit.**

```bash
cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check
git add src/base64.rs src/lib.rs
git commit -m "feat: add hand-rolled base64 encoder for kitty payloads"
```

---

### Task B2: RGBA rasterizer (frame + palette + scale + flip → pixels)

**Files:**
- Create: `src/raster.rs`
- Modify: `src/lib.rs` (`pub mod raster;`)
- Test: `src/raster.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct Rgba { pub w: usize, pub h: usize, pub px: Vec<u8> } // len == w*h*4
  pub fn rasterize(frame: &crate::sprite::Frame, hue: u16, theme: crate::palette::Theme,
                   style: crate::palette::StateStyle, scale: usize, flip: bool) -> Rgba;
  ```
  Transparent roles → alpha 0; opaque roles → `role_color(...)` RGB + alpha 255; each sprite pixel becomes a `scale`×`scale` block; `flip` mirrors on x.

- [ ] **Step 1: Write failing tests** (`src/raster.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{StateStyle, Theme};
    use crate::sprite::parse_species;

    const BLOB: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/test-blob.sprite"));

    fn frame0() -> crate::sprite::Frame {
        let sp = parse_species(BLOB).unwrap();
        sp.states[&crate::agent::AgentStatus::Idle].frames[0].clone()
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
        assert!(r.px.chunks(4).any(|p| p[3] == 0), "some pixel is transparent");
        assert!(r.px.chunks(4).any(|p| p[3] == 255), "some pixel is opaque");
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
```

- [ ] **Step 2: Run to see it fail.** Run: `cargo test -p herdr-pets raster` → FAIL.

- [ ] **Step 3: Implement.**

```rust
//! Rasterize a sprite `Frame` to RGBA pixels for the kitty renderer, reusing
//! the same role->color palette as the half-block path so both look identical.

use crate::palette::{StateStyle, Theme, role_color};
use crate::sprite::Frame;

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
```

- [ ] **Step 4: Run to see it pass.** Run: `cargo test -p herdr-pets raster` → PASS.

- [ ] **Step 5: Gate + commit.**

```bash
cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check
git add src/raster.rs src/lib.rs
git commit -m "feat(raster): rasterize sprite frames to RGBA for kitty rendering"
```

---

### Task B3: Kitty escape-sequence encoders

**Files:**
- Create: `src/kitty.rs`
- Modify: `src/lib.rs` (`pub mod kitty;`)
- Test: `src/kitty.rs`

**Interfaces:**
- Produces:
  ```rust
  // Transmit RGBA image data under image id `id` (chunked base64, f=32), no display.
  pub fn transmit_rgba(id: u32, w: usize, h: usize, rgba: &[u8]) -> String;
  // Place already-transmitted image `id` as placement `pid` at the current cursor.
  pub fn place(id: u32, pid: u32) -> String;
  // Delete a placement (removes the image from screen; keeps stored data).
  pub fn delete_placement(id: u32, pid: u32) -> String;
  // Delete all images (cleanup on teardown).
  pub fn delete_all() -> String;
  // The query used by the capability probe (1x1 image, a=q).
  pub fn probe_query(id: u32) -> String;
  ```
  All emit `\x1b_G<control>;<payload>\x1b\\`. Chunking splits base64 into ≤4096-char chunks with `m=1`/`m=0`.

- [ ] **Step 1: Write failing tests** (`src/kitty.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transmit_wraps_in_apc_and_sets_dimensions() {
        // 1x1 opaque red pixel
        let s = transmit_rgba(7, 1, 1, &[255, 0, 0, 255]);
        assert!(s.starts_with("\x1b_G"));
        assert!(s.ends_with("\x1b\\"));
        assert!(s.contains("a=t"));   // transmit only (no display)
        assert!(s.contains("f=32"));
        assert!(s.contains("s=1"));
        assert!(s.contains("v=1"));
        assert!(s.contains("i=7"));
    }

    #[test]
    fn large_payload_is_chunked_with_m_flags() {
        // 40x40 RGBA = 6400 bytes -> base64 ~8536 chars -> >1 chunk of 4096.
        let s = transmit_rgba(1, 40, 40, &vec![1u8; 40 * 40 * 4]);
        assert!(s.matches("\x1b_G").count() >= 2, "multiple APC chunks");
        assert!(s.contains("m=1"), "non-final chunks set m=1");
        assert!(s.contains("m=0"), "final chunk sets m=0");
    }

    #[test]
    fn place_and_delete_reference_ids() {
        assert!(place(7, 3).contains("a=p") && place(7, 3).contains("i=7") && place(7, 3).contains("p=3"));
        assert!(delete_placement(7, 3).contains("a=d") && delete_placement(7, 3).contains("i=7"));
        assert_eq!(delete_all(), "\x1b_Ga=d,d=A\x1b\\");
        assert!(probe_query(9).contains("a=q") && probe_query(9).contains("i=9"));
    }
}
```

- [ ] **Step 2: Run to see it fail.** Run: `cargo test -p herdr-pets kitty` → FAIL.

- [ ] **Step 3: Implement.**

```rust
//! Kitty graphics protocol escape-sequence builders. We emit these ourselves;
//! herdr forwards them to the outer terminal when its experimental
//! `[experimental] kitty_graphics = true` flag is on (see the design spec).

use crate::base64;

const CHUNK: usize = 4096; // max base64 chars per APC chunk (protocol limit)

fn apc(control: &str, payload: &str) -> String {
    format!("\x1b_G{control};{payload}\x1b\\")
}

/// Transmit RGBA (`f=32`) image data under image id `id`, without displaying it
/// (`a=t`). `q=2` suppresses the terminal's success/failure replies.
pub fn transmit_rgba(id: u32, w: usize, h: usize, rgba: &[u8]) -> String {
    let b64 = base64::encode(rgba);
    let chunks: Vec<&str> = if b64.is_empty() {
        vec![""]
    } else {
        (0..b64.len()).step_by(CHUNK).map(|i| &b64[i..(i + CHUNK).min(b64.len())]).collect()
    };
    let mut out = String::new();
    for (idx, chunk) in chunks.iter().enumerate() {
        let last = idx == chunks.len() - 1;
        let control = if idx == 0 {
            format!("a=t,f=32,s={w},v={h},i={id},q=2,m={}", if last { 0 } else { 1 })
        } else {
            format!("m={}", if last { 0 } else { 1 })
        };
        out.push_str(&apc(&control, chunk));
    }
    out
}

/// Place transmitted image `id` as placement `pid` at the current cursor.
pub fn place(id: u32, pid: u32) -> String {
    apc(&format!("a=p,i={id},p={pid},q=2"), "")
}

/// Delete placement `pid` of image `id` (removes it from screen; keeps data).
pub fn delete_placement(id: u32, pid: u32) -> String {
    apc(&format!("a=d,d=i,i={id},p={pid},q=2"), "")
}

/// Delete all images and placements (teardown / clean exit).
pub fn delete_all() -> String {
    apc("a=d,d=A", "")
}

/// The capability-probe query: transmit+query a 1x1 image under `id` (`a=q`).
pub fn probe_query(id: u32) -> String {
    let b64 = base64::encode(&[0u8, 0, 0]); // 1x1 RGB
    apc(&format!("a=q,i={id},f=24,s=1,v=1"), &b64)
}
```

- [ ] **Step 4: Run to see it pass.** Run: `cargo test -p herdr-pets kitty` → PASS.

- [ ] **Step 5: Gate + commit.**

```bash
cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check
git add src/kitty.rs src/lib.rs
git commit -m "feat(kitty): add kitty graphics escape-sequence builders"
```

---

### Task B4: `PetRenderer` trait + refactor the half-block path behind it

**Files:**
- Modify: `src/render.rs` (extract `HalfBlockRenderer`, define trait)
- Test: `src/render.rs` (existing snapshot tests must still pass unchanged)

**Interfaces:**
- Produces:
  ```rust
  pub trait PetRenderer {
      fn draw(&mut self, frame: &mut ratatui::Frame, herd: &Herd, species: &[Species], theme: Theme);
      fn pet_at_column(&self, herd: &Herd, species: &[Species], strip_w: usize, col: u16) -> Option<usize>;
      fn teardown(&mut self) -> std::io::Result<()> { Ok(()) }
  }
  pub struct HalfBlockRenderer;
  ```
- Consumes: existing `draw_herd`, `pet_at_column` free functions.

- [ ] **Step 1: Introduce the trait + a thin `HalfBlockRenderer` that delegates to the existing free functions.** Add to `src/render.rs`:

```rust
/// A pluggable pet-strip renderer. The simulation is shared; only drawing and
/// hit-testing differ between backends (half-block vs kitty graphics).
pub trait PetRenderer {
    /// Draw the whole strip for this frame (pet band + overlays + `+N`).
    fn draw(&mut self, frame: &mut Frame, herd: &Herd, species: &[Species], theme: Theme);
    /// The visible pet under terminal column `col`, if any (for hover/click).
    fn pet_at_column(&self, herd: &Herd, species: &[Species], strip_w: usize, col: u16) -> Option<usize>;
    /// Release backend resources (kitty: delete transmitted images). Default no-op.
    fn teardown(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The universal half-block renderer (ratatui `▀▄` cells).
pub struct HalfBlockRenderer;

impl PetRenderer for HalfBlockRenderer {
    fn draw(&mut self, frame: &mut Frame, herd: &Herd, species: &[Species], theme: Theme) {
        draw_herd(frame, herd, species, theme);
    }
    fn pet_at_column(&self, herd: &Herd, species: &[Species], strip_w: usize, col: u16) -> Option<usize> {
        pet_at_column(herd, species, strip_w, col)
    }
}
```

- [ ] **Step 2: Thread the renderer through `run_loop`.** Change `run_loop` to take `renderer: &mut dyn PetRenderer` and call `renderer.draw(f, &herd, species, theme)` inside the `terminal.draw` closure, and `renderer.pet_at_column(...)` for hover/click. `run` constructs a `HalfBlockRenderer` for now and passes it. Keep `draw_caption` as-is (called alongside `renderer.draw`). Signature:

```rust
fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    rx: Receiver<Vec<Agent>>,
    species: &[Species],
    theme: Theme,
    focus: &dyn HerdrCli,
    reduced_motion: bool,
    renderer: &mut dyn PetRenderer,
) -> io::Result<()> where io::Error: From<B::Error> { /* ... */ }
```

- [ ] **Step 3: Run the whole render suite (no behavior change expected).**

Run: `cargo test -p herdr-pets render`
Expected: PASS with **no snapshot changes** (the half-block output is identical; we only moved calls behind a trait). If a snapshot changed, the refactor altered behavior — revert and re-extract without changing draw logic.

- [ ] **Step 4: Add a trait-level test** proving the seam:

```rust
#[test]
fn half_block_renderer_matches_the_free_function() {
    let species = vec![parse_species(BLOB).unwrap()];
    let herd = fixed_herd(&[AgentStatus::Working, AgentStatus::Blocked]);
    let mut via_trait = Terminal::new(TestBackend::new(60, 8)).unwrap();
    via_trait.draw(|f| HalfBlockRenderer.draw(f, &herd, &species, Theme::Dark)).unwrap();
    let mut via_fn = Terminal::new(TestBackend::new(60, 8)).unwrap();
    via_fn.draw(|f| draw_herd(f, &herd, &species, Theme::Dark)).unwrap();
    assert_eq!(format!("{}", via_trait.backend()), format!("{}", via_fn.backend()));
}
```

Run: `cargo test -p herdr-pets half_block_renderer_matches` → PASS.

- [ ] **Step 5: Gate + commit.**

```bash
cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check
git add src/render.rs
git commit -m "refactor(render): put the half-block path behind a PetRenderer trait"
```

---

### Task B5: Terminal capability probe (kitty support detection)

**Files:**
- Create: `src/caps.rs`
- Modify: `src/lib.rs` (`pub mod caps;`)
- Test: `src/caps.rs`

**Interfaces:**
- Produces:
  ```rust
  pub trait TerminalCaps { fn supports_kitty_graphics(&mut self) -> bool; }
  pub struct RealCaps { /* reads the tty with a deadline */ }
  impl RealCaps { pub fn new() -> Self; }
  // A test double:
  pub struct FakeCaps { pub supported: bool }
  ```
  `RealCaps::supports_kitty_graphics` writes `kitty::probe_query(id)` to stdout in raw mode and reads stdin for an `\x1b_G...i=<id>...\x1b\\` reply within ~150ms; true iff a matching reply arrives.

- [ ] **Step 1: Write failing tests** (fakeable behavior only — no real tty in tests):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fake_reports_configured_support() {
        assert!(FakeCaps { supported: true }.supports_kitty_graphics());
        assert!(!FakeCaps { supported: false }.supports_kitty_graphics());
    }
    #[test]
    fn reply_matcher_accepts_only_matching_image_id() {
        // Pure parser used by RealCaps, unit-tested without a terminal.
        assert!(reply_confirms(b"\x1b_Gi=31,OK\x1b\\", 31));
        assert!(!reply_confirms(b"\x1b_Gi=99;OK\x1b\\", 31));
        assert!(!reply_confirms(b"garbage", 31));
    }
}
```

- [ ] **Step 2: Run to see it fail.** Run: `cargo test -p herdr-pets caps` → FAIL.

- [ ] **Step 3: Implement.** Split the pure reply matcher (tested) from the I/O (not unit-tested; exercised live):

```rust
//! Runtime detection of whether the outer terminal (through herdr) will render
//! kitty graphics. Self-correcting: if herdr's experimental flag is off, the
//! query is swallowed and no reply arrives, so we report unsupported and the
//! caller falls back to half-blocks. The tty I/O lives behind this trait so
//! tests never touch a real terminal.

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

/// Whether the current terminal supports the kitty graphics protocol.
pub trait TerminalCaps {
    fn supports_kitty_graphics(&mut self) -> bool;
}

/// True if `buf` contains a kitty graphics reply naming image id `id`
/// (`\x1b_G...i=<id>...\x1b\`). Pure; unit-tested.
pub fn reply_confirms(buf: &[u8], id: u32) -> bool {
    let text = String::from_utf8_lossy(buf);
    text.split("\x1b_G")
        .skip(1)
        .any(|seg| seg.split(['\x1b', ';', ',']).any(|tok| tok == format!("i={id}")))
}

/// Reads the real tty. Assumes raw mode is already enabled by the caller
/// (the render loop enables it); writes the probe and polls stdin briefly.
pub struct RealCaps {
    id: u32,
    timeout: Duration,
}

impl RealCaps {
    pub fn new() -> Self {
        Self { id: 0x7E51, timeout: Duration::from_millis(150) }
    }
}

impl Default for RealCaps {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalCaps for RealCaps {
    fn supports_kitty_graphics(&mut self) -> bool {
        let query = crate::kitty::probe_query(self.id);
        if io::stdout().write_all(query.as_bytes()).is_err() || io::stdout().flush().is_err() {
            return false;
        }
        // Poll stdin for a reply until the deadline. crossterm's event stream
        // is already in use by the caller after this returns, so read raw here
        // before the loop starts.
        let deadline = Instant::now() + self.timeout;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 256];
        let mut stdin = io::stdin();
        while Instant::now() < deadline {
            // Non-blocking-ish: rely on crossterm::event::poll for readiness.
            if crossterm::event::poll(Duration::from_millis(20)).unwrap_or(false) {
                // Drain available bytes via a raw read.
                if let Ok(n) = stdin.read(&mut chunk) {
                    buf.extend_from_slice(&chunk[..n]);
                    if reply_confirms(&buf, self.id) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Test double.
#[cfg(any(test, feature = "test-support"))]
pub struct FakeCaps {
    pub supported: bool,
}
#[cfg(any(test, feature = "test-support"))]
impl TerminalCaps for FakeCaps {
    fn supports_kitty_graphics(&mut self) -> bool {
        self.supported
    }
}
```

> **Implementer note (verify live):** the exact raw-stdin read strategy under crossterm raw mode may need adjustment (crossterm consumes stdin via its event queue). If `event::poll` + `stdin.read` conflicts, instead read the reply by pulling `crossterm::event::read()` events and reconstructing the APC bytes, or do the probe *before* the event loop starts. Keep `reply_confirms` pure and tested regardless. This is the one task with a live-verification step — confirm the probe returns true with `kitty_graphics=true` + reattached, and false with it off, in a scratch pane before finalizing.

- [ ] **Step 4: Run to see it pass.** Run: `cargo test -p herdr-pets caps` → PASS (pure tests).

- [ ] **Step 5: Gate + commit.**

```bash
cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check
git add src/caps.rs src/lib.rs
git commit -m "feat(caps): detect kitty graphics support via a self-correcting probe"
```

---

### Task B6: Config — `renderer` and `pet_scale` knobs

**Files:**
- Modify: `src/config.rs`
- Test: `src/config.rs`

**Interfaces:**
- Produces: `Config.renderer: RendererKind` (`Auto` default | `Kitty` | `HalfBlock`) and `Config.pet_scale: usize` (default `7`).
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum RendererKind { Auto, Kitty, HalfBlock }
  ```

- [ ] **Step 1: Write failing tests** (extend `src/config.rs` tests):

```rust
#[test]
fn parses_renderer_and_scale() {
    let c = Config::from_toml_str("renderer = kitty\npet_scale = 5\n");
    assert_eq!(c.renderer, RendererKind::Kitty);
    assert_eq!(c.pet_scale, 5);
}
#[test]
fn renderer_defaults_to_auto_and_scale_to_seven() {
    let c = Config::default();
    assert_eq!(c.renderer, RendererKind::Auto);
    assert_eq!(c.pet_scale, 7);
}
#[test]
fn unknown_renderer_value_falls_back_to_auto() {
    let c = Config::from_toml_str("renderer = hologram\n");
    assert_eq!(c.renderer, RendererKind::Auto);
}
```

- [ ] **Step 2: Run to see it fail.** Run: `cargo test -p herdr-pets config` → FAIL.

- [ ] **Step 3: Implement.** Add the enum, fields, defaults, and parse arms:

```rust
/// Which rendering backend to use. `Auto` probes for kitty support and falls
/// back to half-blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererKind {
    Auto,
    Kitty,
    HalfBlock,
}
```
Add to `Config`: `pub renderer: RendererKind,` and `pub pet_scale: usize,`. In `Default`: `renderer: RendererKind::Auto, pet_scale: 7,`. In `from_toml_str`, add match arms:

```rust
"renderer" => {
    cfg.renderer = match val {
        "kitty" => RendererKind::Kitty,
        "half-block" | "half_block" | "halfblock" => RendererKind::HalfBlock,
        _ => RendererKind::Auto, // "auto" or anything unrecognized
    };
}
"pet_scale" => {
    if let Ok(v) = val.parse::<usize>() {
        cfg.pet_scale = v.clamp(1, 24);
    }
}
```
Update the two existing `assert_eq!(..., Config { ... })` tests to include the new fields.

- [ ] **Step 4: Run to see it pass.** Run: `cargo test -p herdr-pets config` → PASS.

- [ ] **Step 5: Gate + commit.**

```bash
cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check
git add src/config.rs
git commit -m "feat(config): add renderer and pet_scale knobs"
```

---

### Task B7: `KittyRenderer` — image cache, placement, hit-testing, teardown

**Files:**
- Create: `src/kitty_render.rs`
- Modify: `src/lib.rs` (`pub mod kitty_render;`), `src/render.rs` (import for the trait)
- Test: `src/kitty_render.rs`

**Interfaces:**
- Consumes: `raster::rasterize`, `kitty::{transmit_rgba, place, delete_placement, delete_all}`, `Herd`, `Species`, `Pet`, `PetRenderer` trait, `Config.pet_scale`, cell pixel size.
- Produces: `pub struct KittyRenderer` implementing `PetRenderer`. It writes escapes to an injected `io::Write` sink (Real = stdout; test = `Vec<u8>`), so encoding is unit-testable.

```rust
pub struct KittyRenderer {
    scale: usize,
    cell_px: (u16, u16),        // (w,h) px per cell; queried, or a default
    out: Box<dyn std::io::Write + Send>,
    cache: std::collections::HashMap<ImgKey, u32>, // (species,status,frame,flip,hue) -> image id
    placements: std::collections::HashMap<String, u32>, // terminal_id -> placement id
    next_id: u32,
}
type ImgKey = (usize, crate::agent::AgentStatus, usize, bool, u16);
```

- [ ] **Step 1: Write failing tests** (drive draw() with a `Vec<u8>` sink and assert on the escapes):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
    use crate::herd::{Herd, Lcg};
    use crate::identity::identity_for;
    use crate::palette::Theme;
    use crate::pet::Pet;
    use crate::sprite::parse_species;

    const BLOB: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/test-blob.sprite"));

    fn one_working_herd() -> Herd {
        let mut h = Herd::new();
        h.pets.push(Pet::new("t1".into(), identity_for("t1", 1), AgentStatus::Working, 4.0));
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
        assert!(!second.contains("a=t"), "same frame reuses the cached image (no re-transmit)");
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
```

> The plan provides `SharedSink` (an `Rc<RefCell<Vec<u8>>>`-style `io::Write` you can read back) and `for_test` / `draw_to_sink` constructors as test-only helpers in this module — implement them in Step 3 alongside the real code.

- [ ] **Step 2: Run to see it fail.** Run: `cargo test -p herdr-pets kitty_render` → FAIL.

- [ ] **Step 3: Implement `KittyRenderer`.** Core logic:
  - `draw`: compute the visible set + z-order exactly like `draw_herd` (reuse `visible_and_hidden` + the `priority` sort). For each visible pet: pick its `Frame` (`sp.states[status].frames[pet.frame_index(len)]`), key = `(species_index, status, frame_index, pet.facing_left, hue)`; if not cached, `rasterize` + `transmit_rgba(new_id, ...)` and store; move the cursor to the pet's cell (`\x1b[<row>;<col>H`, col from `pet.x`, row bottom-anchored in the band), then `place(image_id, placement_id)`; delete the pet's previous placement (draw-then-delete). Track `placements[terminal_id]`.
  - Remove placements for pets no longer present (departed/hidden): delete them.
  - `pet_at_column`: footprint width in cells = `ceil(frame_w * scale / cell_px.0)`; a pet covers `[col0, col0 + width)`; topmost by priority wins (same tie-break as `render::pet_at_column`).
  - `teardown`: write `kitty::delete_all()`.
  - `cell_px`: `RealCaps`/a cell-size query (`CSI 14 t`) at construction; fall back to `(8, 16)` if unknown. (Provide a `KittyRenderer::new(scale, cell_px, out)` and a `for_test` that injects both.)

Follow `rust-error-handling` (propagate write errors with `?`; the render loop already swallows a failed frame rather than crashing). Keep methods small.

- [ ] **Step 4: Run to see it pass.** Run: `cargo test -p herdr-pets kitty_render` → PASS.

- [ ] **Step 5: Gate + commit.**

```bash
cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check
git add src/kitty_render.rs src/lib.rs src/render.rs
git commit -m "feat(kitty): add the KittyRenderer backend (cache, place, hit-test)"
```

---

### Task B8: Wire renderer selection, docs, and the decision log

**Files:**
- Modify: `src/render.rs` (`run` selects the backend), `src/main.rs` (pass config), `src/caps.rs` usage
- Modify: `README.md` (document `renderer`, `pet_scale`, and the herdr prerequisite)
- Modify: `docs/decisions.md` (record this work + that it supersedes v3 slimming)
- Test: `src/render.rs` (selection unit test with `FakeCaps`)

**Interfaces:**
- Consumes: `Config.renderer`, `Config.pet_scale`, `TerminalCaps`, `HalfBlockRenderer`, `KittyRenderer`.
- Produces: `pub fn select_renderer(kind: RendererKind, caps: &mut dyn TerminalCaps, scale: usize, out) -> Box<dyn PetRenderer>`.

- [ ] **Step 1: Write the failing selection test** (`src/render.rs` tests):

```rust
#[test]
fn auto_picks_kitty_when_supported_else_half_block() {
    use crate::caps::FakeCaps;
    use crate::config::RendererKind;
    let is_kitty = |r: &Box<dyn PetRenderer>| /* downcast-free check */ r.backend_name() == "kitty";
    let mut yes = FakeCaps { supported: true };
    assert!(is_kitty(&select_renderer(RendererKind::Auto, &mut yes, 7)));
    let mut no = FakeCaps { supported: false };
    assert!(!is_kitty(&select_renderer(RendererKind::Auto, &mut no, 7)));
    // Forced modes ignore the probe:
    let mut yes2 = FakeCaps { supported: true };
    assert!(!is_kitty(&select_renderer(RendererKind::HalfBlock, &mut yes2, 7)));
}
```

> Add a trivial `fn backend_name(&self) -> &'static str` to `PetRenderer` (`"half-block"` / `"kitty"`) so selection is assertable without downcasting.

- [ ] **Step 2: Run to see it fail.** Run: `cargo test -p herdr-pets auto_picks_kitty` → FAIL.

- [ ] **Step 3: Implement `select_renderer`** in `src/render.rs`:

```rust
/// Choose the backend: forced kinds win; `Auto` probes and falls back to
/// half-blocks when kitty graphics are unavailable (herdr flag off, non-kitty
/// terminal, etc.).
pub fn select_renderer(
    kind: crate::config::RendererKind,
    caps: &mut dyn crate::caps::TerminalCaps,
    scale: usize,
) -> Box<dyn PetRenderer> {
    use crate::config::RendererKind::*;
    let use_kitty = match kind {
        HalfBlock => false,
        Kitty => true,
        Auto => caps.supports_kitty_graphics(),
    };
    if use_kitty {
        Box::new(crate::kitty_render::KittyRenderer::new_stdout(scale))
    } else {
        Box::new(HalfBlockRenderer)
    }
}
```
Add `backend_name` to the trait + both impls. In `run`, after `enable_raw_mode()` (so the probe can read the tty), build `caps = RealCaps::new()`, call `select_renderer(cfg.renderer, &mut caps, cfg.pet_scale)`, and pass `renderer.as_mut()` into `run_loop`; call `renderer.teardown()` in the cleanup block alongside `disable_raw_mode`. Thread `cfg` (or the two fields) from `main.rs` `render` arm into `render::run` (extend `run`'s signature to accept `renderer_kind` + `pet_scale`, or the whole `Config`).

- [ ] **Step 4: Run selection test + full suite.** Run: `cargo test -p herdr-pets` → PASS.

- [ ] **Step 5: Document.** In `README.md`, add a "Rendering" section:
  - `renderer = "auto" | "kitty" | "half-block"` (default `auto`) and `pet_scale` (default 7) in the config table.
  - The kitty upgrade prerequisite: the outer terminal must support the kitty graphics protocol (e.g. Ghostty, kitty), and **herdr** must have `[experimental] kitty_graphics = true` in `~/.config/herdr/config.toml`, followed by `herdr server reload-config` and a client **detach + reattach**. Note it's experimental and that half-block is the automatic fallback.

- [ ] **Step 6: Update `docs/decisions.md`.** Append a dated entry: kitty backend added as the opt-in upgrade; the research + live spikes that justified it; that it supersedes the Improvement-3 v3 12×5 slimming (sprites restored to 16×14); the accepted taller half-block fallback; dependence on herdr's experimental flag.

- [ ] **Step 7: Gate + commit.**

```bash
cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check
git add src/render.rs src/main.rs src/caps.rs README.md docs/decisions.md
git commit -m "feat(render): select kitty vs half-block backend; document the upgrade"
```

- [ ] **Step 8: Live verification (maintainer-assisted).** In a scratch herdr pane (kitty flag on + reattached), run the built `render` binary and confirm: pets are small/crisp/detailed, idle pets are still, working pets amble slowly facing travel direction, clicking focuses the agent, hovering shows the label. Then set `renderer = "half-block"` (or disable the herdr flag) and confirm the full-detail half-block fallback renders. Capture a screenshot for the maintainer.

---

## Self-review (completed)

- **Spec coverage:** §3 seam → B4; §4 detection → B5, B8; §5 behavior → A2, A3; §6 sprites → A1; §7 kitty details → B1, B2, B3, B7; §8 config → B6; §9 interactivity → B4 (half-block), B7 (kitty hit-test), B8; §10 testing → tests in every task; §11 risks → B5 note + B8 live check; §12 sequencing → Step A / Step B; §13 DoD → B8. No gaps.
- **Placeholders:** none — every code step has real code; the two "verify live" notes (B5 probe I/O, B8) are explicit verification steps, not missing content.
- **Type consistency:** `PetRenderer` (draw/pet_at_column/teardown/backend_name), `RendererKind` (Auto/Kitty/HalfBlock), `Rgba`, `rasterize`, `kitty::{transmit_rgba,place,delete_placement,delete_all,probe_query}`, `TerminalCaps`/`RealCaps`/`FakeCaps`, `Pet.facing_left`/`set_facing_from_dx`, `select_renderer` — names consistent across tasks.
