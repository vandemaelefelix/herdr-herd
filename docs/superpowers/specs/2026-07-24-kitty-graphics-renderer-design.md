# Design — Kitty-graphics rendering backend (small, crisp, detailed pets)

**Date:** 2026-07-24
**Status:** Approved (design conversation with maintainer), ready for implementation plan.
**Supersedes:** the Improvement-3 v3 sprite slimming (12×5 hand-drawn sprites +
`PET_PX_H=6` band). See `docs/decisions.md` "Improvement 3" entries.

## 1. Why

The maintainer wants pets that are **small on screen but keep the full detail**
of the traced artifact sprites (Artifact `85ac4f4a`; the 16×14 v2 art). In a
terminal, half-block rendering (`▀▄`) cannot do this: each sprite pixel is
locked to 1 char cell wide × ½ cell tall, so a 16×14 sheep is intrinsically
~16 cols × 7 rows. "Smaller" in half-blocks can only mean "fewer pixels" =
less detail. That tradeoff drove the v3 redraw, which the maintainer disliked
(it discarded the artifact art).

**Research + live spikes (2026-07-24) changed the picture.** The full option
space was investigated (custom fonts, image protocols, environment ground
truth). Findings:

- The maintainer's terminal is **Ghostty**, which natively supports the **kitty
  graphics protocol** (true pixel images: any size, full color, crisp, animatable).
- herdr sits between the plugin and Ghostty. herdr **vendors `libghostty-vt`**
  and has first-class kitty-graphics support (incl. the unicode-placeholder /
  virtual-placement feature that survives a multiplexer), gated behind an
  **experimental, off-by-default** config flag:
  ```toml
  [experimental]
  kitty_graphics = true
  ```
  in `~/.config/herdr/config.toml`. It requires `herdr server reload-config`
  **and a client detach+reattach** (rendering is client-side) to take effect.
- Custom fonts render smooth-not-crisp and are fiddly/unproven on Ghostty.
  Sixel is a dead end (Ghostty declined it). Half-blocks stay crisp but can't be
  made small. **Kitty graphics is the only path to small + crisp + detailed.**

**Proven live (throwaway spikes in a herdr pane):**
1. A 4-color image renders through herdr → Ghostty once the flag is on + client
   reattached (before that, herdr silently drops the escape).
2. The **actual v2 artifact sheep/goat** rasterized with the plugin's palette
   render crisply at small sizes; the maintainer chose **scale ≈ 7** (7 image px
   per sprite pixel) for the standing sheep.
3. **Animation is smooth through herdr's passthrough**: a walking sheep that
   roams and flips to face its direction of travel had no flicker or trails at
   ~12 fps, drawing each frame and deleting the previous image id.

This aligns with `GOAL.md`'s locked decision: *"Universal first … Fancier
rendering (e.g. Kitty graphics) is only ever an opt-in upgrade on top, never a
requirement."* This spec brings that deferred stretch forward.

See memory `herdr-pets-kitty-graphics-works` for the durable summary.

## 2. Goals / non-goals

**Goals**
- Add a **kitty-graphics rendering backend** that draws the pets as small,
  crisp, full-color, animated images from the detailed artifact sprites.
- Keep the **half-block renderer** as the universal fallback; select between
  them automatically, safely.
- Restore the **16×14 v2 artifact sprites** as the single sprite source.
- Add the maintainer-requested behavior: **idle pets are stationary**, **only
  working pets amble** (slowly, staying clickable), and **pets face their
  direction of travel** — in both backends.
- No new runtime crate dependencies (honor the existing constraint).
- Keep the green gate: `cargo test -p herdr-pets && cargo clippy -p herdr-pets
  --all-targets -- -D warnings && cargo fmt --check`.

**Non-goals**
- Not making herdr itself change; we rely on herdr's existing (experimental)
  flag. If it's off/unavailable we fall back — we do not try to force it.
- No sixel or custom-font backends.
- No new pet behaviors beyond facing + idle-stationary (no gameplay, per GOAL).
- No auto-tuning of size to cell metrics in v1 (scale is a configured default).

## 3. Architecture — one simulation, two draw backends

The simulation is already rendering-agnostic. Per tick it yields, for each
visible pet: position `x`, animation `phase` → frame index, `status`,
`identity` (species + hue), and a motion offset. Nothing about this is tied to
half-blocks.

Introduce a small seam:

```
trait PetRenderer {
    // Draw the whole strip for this frame: pet band + overlays + caption + +N.
    fn draw(&mut self, frame_ctx, herd, species, theme) -> io::Result<()>;
    // Map a terminal column to the pet drawn under it (for click/hover).
    fn pet_at_column(&self, herd, species, strip_w, col) -> Option<usize>;
    // Release any resources (kitty: delete transmitted images) on shutdown.
    fn teardown(&mut self) -> io::Result<()>;
}
```

- **`HalfBlockRenderer`** — the current `render.rs` drawing path, refactored
  behind the trait. ratatui + `▀▄` cells. Behavior unchanged except the shared
  behavior additions (§5) and the sprite/size restore (§6).
- **`KittyRenderer`** — emits kitty graphics escapes (§7). Bypasses ratatui for
  the pet band (images are out-of-band); overlays/caption/`+N` stay as text
  (ratatui or direct writes).

`render::run` / `run_loop` keeps owning the simulation (reconcile, `simulate_tick`,
mouse handling, quit) and calls the active renderer. The renderer is chosen once
at startup (§4). The `run_loop`'s existing `terminal.draw(...)` closure becomes
a call through the trait; the kitty backend may draw outside ratatui's buffer,
so the loop must accommodate a renderer that writes escapes directly (keep a
handle to stdout / the backend).

**Testability:** both renderers are constructed behind the trait; the kitty
backend's capability probe and stdout are injected (Real/Fake) per
`rust-testability-seams`, so tests never touch a real terminal.

## 4. Detection & fallback

New config key `renderer` (§8): `"auto"` (default) | `"kitty"` | `"half-block"`.

- **`half-block`** — always the current renderer. Always safe.
- **`kitty`** — force the kitty backend (maintainer opt-in, skip probing).
- **`auto`** (default) — run a **capability probe** at startup; use kitty if it
  succeeds, else half-block.

**Capability probe.** Send a kitty graphics *query* (transmit a 1×1 image with
`a=q`, a chosen image id) and read stdin for the `\e_Gi=<id>;OK\e\` reply with a
short deadline (~100–200 ms) in raw mode. This is **self-correcting through
herdr**: flag off → herdr swallows the escape → no reply → half-block; flag on +
Ghostty → reply → kitty. Behind a `TerminalCaps` trait (Real reads the tty;
Fake returns a scripted verdict) so it's unit-testable and can't hang tests.

Rationale for probing over reading herdr's config: the probe tests the *actual
render path end-to-end* (flag + outer-terminal support + reattach state) rather
than inferring from a file whose location/format we don't own.

## 5. Shared behavior changes (both backends)

1. **Idle → stationary; only working roams.** In `herd::step`, set the roam
   probability to 0 for every status except `Working` (today idle/done/unknown
   drift at 0.35; blocked already holds at 0). Non-working pets keep their
   in-place motion (breathe/wag/paw/shake) but do not change `x`.
2. **Slow, clickable amble.** Lower the working roam `speed` and/or target-change
   rate so movement is a gentle "working" signal (matching the approved demo),
   never fast enough to make the pet hard to click.
3. **Face direction of travel.** Track each pet's horizontal direction (sign of
   `x` change, or `target_x - x`) on the `Pet`. Sprites are authored facing one
   way (head/eye on the right = faces right); when moving left, **mirror the
   frame horizontally**. HalfBlock mirrors sprite cells at blit time; Kitty
   rasterizes a flipped variant (cached separately). Keep the last non-zero
   direction so a stationary pet keeps a stable facing.

These live in the shared simulation / `Pet`, so both renderers get them.

## 6. Sprites — restore the artifact art as the single source

- Replace the current hand-drawn 12×5 `sprites/sheep.sprite` and
  `sprites/goat.sprite` with the **16×14 v2 artifact art** recovered from
  `git show 2dc1378:sprites/sheep.sprite` (and `goat.sprite`). One source feeds
  both renderers.
- **Revert the v3 half-block sizing** so the fallback shows full detail:
  `PET_PX_H` back to the sprite height (≈14), the `h <= 5` guard in `sprite.rs`
  back to the sprite height, band-row math in `render.rs`, and the hop/shake
  caps in `anim.rs` back to what the taller band allows. (This restores the
  Improvement-3 v2 state for the half-block path.)
- **Accepted tradeoff (maintainer-confirmed):** the half-block *fallback* strip
  is therefore the "full-detail but taller" (~7–9 row) sheep again. The
  maintainer's own experience is the crisp small kitty path; the taller fallback
  only affects non-kitty terminals/users, and on tall displays herdr already
  forces ~9 rows regardless. A slimmer fallback (separate sprite set or
  downscale) is explicitly deferred.

## 7. Kitty backend details

**Rasterization.** Reuse `palette::role_color` (per-agent hue, theme, dim/ghost)
to turn a `Frame` (role grid) into RGBA pixels; transparent roles → alpha 0.
Scale each sprite pixel to an S×S block (nearest-neighbor → crisp). Pure Rust,
no crate.

**base64.** Hand-roll the ~20 lines of standard base64 the protocol needs (no
new runtime dependency).

**Image lifecycle (transmit-once, place-many).**
- Cache transmitted images keyed by `(species, status, frame_index, facing,
  hue_bucket)`. Transmit lazily on first use with a stable kitty image id;
  reuse afterwards. This is lighter than the spike's per-frame re-transmit.
- Each tick, per visible pet: create/replace a **placement** at the pet's cell
  position for the current frame's image id; delete the pet's previous placement
  (draw-then-delete-previous, as validated, to avoid flicker/trails).
- On pet removal / overflow-hide / shutdown (`teardown`): delete that pet's
  image(s)/placements. Delete-all on exit to leave the pane clean.

**Sizing & footprint.** Rasterize at `pet_scale` px per sprite pixel (config §8,
default **7** — the validated look). Place at the image's native pixel size (as
the spike did). To get a deterministic **cell footprint** (needed for layout and
hit-testing), query the terminal's cell pixel size once at startup (`CSI 14 t`,
which Ghostty answers); footprint cells = ceil(image_px / cell_px). If the query
fails, fall back to a sane default cell size. (Placing with explicit cell
dimensions via kitty `c=`/`r=` is an alternative if native sizing proves
inconsistent across terminals — contingency, not v1.)

**Coordinate mapping.** Pet `x` (already ~columns in the half-block model) maps
to a start cell column; vertical placement bottom-anchors in the band like the
half-block path. Overlaps resolved by draw order = z-priority (blocked on top),
same as today.

**Redraw persistence.** Direct placement worked through herdr in the spikes. If
herdr repaints ever drop a placed image, the fallback is the unicode-placeholder
/ virtual-placement mode libghostty supports (anchors images to cells). Treat
this as a contingency, not v1 scope; note it as a risk (§11).

**Overlays / caption / +N.** Stay as text cells (bubbles/badges/`+N`, hover
caption) exactly as today, drawn in the reserved lane / bottom row. No occlusion
of the pet (image band is separate).

## 8. Config additions

Extend the hand-rolled tolerant parser in `config.rs` (no `toml` dep):
- `renderer` — `"auto"` (default) | `"kitty"` | `"half-block"`.
- `pet_scale` — kitty pet size in image px per sprite pixel; default **7**.
  Ignored by half-block.
- Existing knobs (`enabled`, `strip_rows`, `sweep_interval_ms`,
  `reduced_motion`) unchanged. `reduced_motion` also freezes the kitty amble
  (it already gates `simulate_tick`).

Unknown/malformed values degrade to defaults (existing behavior). README + the
plugin's example config document the `renderer` knob and the herdr
`[experimental] kitty_graphics = true` prerequisite for the kitty upgrade.

## 9. Interactivity (preserved — GOAL requirement)

- **Click → focus agent** and **hover → label**: `pet_at_column` moves behind
  the renderer trait. Kitty computes the hit range from each pet's cell footprint
  (start column + placement `c=` width); half-block keeps its current 1px=1col
  logic. Topmost (highest priority) pet wins overlaps, as today.
- Mouse handling in `run_loop` is unchanged; it just calls the trait method.

## 10. Testing strategy

- **Simulation** (herd/anim/pet): existing unit tests, plus new tests for
  idle-stationary (non-working pets keep `x`), working-only roam, and
  facing-direction (direction tracking + stable last-facing).
- **Half-block renderer**: existing `insta` snapshot tests still pass (updated
  for the restored 16×14 sprites + facing).
- **Kitty renderer**: unit-test the escape-sequence **encoding** against known
  byte strings (rasterize → base64 → `\e_G…` control data), image-cache
  keying/reuse, placement + delete-previous sequencing, and `teardown` cleanup.
  All via injected stdout (a `Vec<u8>` sink), no real terminal.
- **Capability probe**: `TerminalCaps` Fake returns supported/unsupported;
  assert `auto` picks the right backend and never blocks.

## 11. Risks & dependencies

- **Experimental herdr flag.** Kitty rendering requires `[experimental]
  kitty_graphics = true` + reattach. Off by default; the plugin cannot set it.
  Mitigation: `auto` falls back to half-block; docs tell users how to enable it.
  Accepted by maintainer.
- **Client reattach requirement.** The flag takes effect only after a client
  detach+reattach (rendering is client-side). Documented; not automatable.
- **Per-terminal / non-Ghostty.** Only kitty-graphics-capable outer terminals
  benefit; everyone else gets the (full-detail) half-block fallback.
- **Redraw persistence** through herdr under heavy repaint/resize is proven for
  the common case but not exhaustively; virtual-placement mode is the
  contingency (§7).
- **Animation cost** at scale (many pets) — mitigated by transmit-once caching;
  worst case matches the proven per-frame path.

## 12. Sequencing (for the implementation plan)

Cohesive feature; land in two safe steps so value ships even if the second slips:

- **Step A — sprites + shared behavior (universal).** Restore 16×14 artifact
  sprites; revert half-block sizing to full detail; add facing + idle-stationary
  + slow amble. Ships a coherent improvement on every terminal.
- **Step B — kitty backend (the headline).** Renderer seam; `KittyRenderer`;
  capability probe + `renderer` config; image lifecycle; hit-testing; docs.

## 13. Definition of done

- `renderer = "kitty"` (or `auto` with the herdr flag on) draws small, crisp,
  full-detail, animated artifact pets in the maintainer's Ghostty; idle pets are
  still, working pets amble slowly and face their travel direction; click/hover
  still work.
- `renderer = "half-block"` (or `auto` without the flag) renders the full-detail
  half-block pets and is unchanged in behavior otherwise.
- No new runtime dependencies; green gate stays green; README/example config
  document the knob + herdr prerequisite; `docs/decisions.md` updated.

## References
- `GOAL.md` (universal-first; kitty as opt-in upgrade; interactivity; stable identity).
- `docs/decisions.md` (Improvement 3 sprite saga this supersedes; Phase 3 non-destructive injection).
- Memory: `herdr-pets-kitty-graphics-works`.
- Recovered sprites: `git show 2dc1378:sprites/sheep.sprite`, `…:goat.sprite`.
- Spikes (scratchpad, throwaway): `kitty_test.py`, `sheep_kitty.py`, `sheep_anim.py`, `sheep_anim2.py`.
