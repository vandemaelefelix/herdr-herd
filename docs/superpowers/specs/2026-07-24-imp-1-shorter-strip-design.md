# Improvement 1 — Shorter strip (design)

**Date:** 2026-07-24
**Kind:** post-roadmap improvement (all five phases Done)
**Status:** approved (autonomous run — brainstorming gate waived), pre-implementation
**Resolves against:** [`GOAL.md`](../../../GOAL.md) — "Slim… Unobtrusive… it should be
possible to forget it's a pane."

## 1. Goal & exit criteria

The strip is **too tall**. Cut its vertical footprint to **roughly half** while
keeping every pet fully legible and the new size looking intentional.

**Exit criteria:**
- The pet pixel band is **halved**: `PET_PX_H` 12 → **6** (3 half-block rows).
- The strip is **4 rows** total (3 pixel rows + 1 caption row), down from 7:
  `config.strip_rows` default 7 → **4**, `place::TARGET_ROWS` 7 → **4**.
- The shipped species (`sheep`, `goat`) are redrawn to fit the 6-px budget and
  still read as their animal, in every state.
- The sprite height guard in `sprite.rs` tracks the new budget (`h <= 6`).
- Snapshots regenerated; README config table + example reflect the new default.
- Gate green: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`.

## 2. Scope

Pure **vertical-footprint** change. **In:** the height constants, the two
shipped sprites (redrawn shorter, same look/species), the height guard, docs,
snapshots. **Out:** the sprite *art redesign* and icon-occlusion work — those are
Improvement 3, which builds on this budget. Hover-label and controller changes
are Improvements 2 and 4. Width, capacity math, and the caption row are
unchanged (capacity derives from species width, which is untouched).

## 3. Grounding facts (from the code)

- `render::PET_PX_H = 12` (6 half-block rows) is the pixel canvas height; the pet
  band. `draw_pixels` packs 2 px/row via `▀`/`▄`.
- `config::Config.strip_rows` default `7`; the controller passes it as
  `target_rows` to `inject_strip`, which feeds `slim_ratio(rows, target_rows)`.
- `place::TARGET_ROWS = 7` is the on-demand `place` strip height (7 = 6 px rows +
  1 caption), independent of config by design (documented).
- `sprite.rs::every_embedded_species_is_valid` asserts `h <= 12` ("the 6-row
  budget"); every state/frame of a species must share one `(w, h)`.
- The caption is drawn on the strip's **bottom** row (`draw_caption`, own row so
  hover never shifts the herd). Overlays (`Zz`/`!`/`?`) draw on the **top** row.
- Sheep + goat are currently `16 × 12`. Halving height → `16 × 6` (width kept, so
  capacity/hit-testing math is unchanged).

## 4. Design

Three constants drop, two sprites shrink, one guard tracks the budget.

| Unit | Change |
|---|---|
| `render.rs` | `PET_PX_H` 12 → 6. |
| `config.rs` | `strip_rows` default 7 → 4 (struct `Default`, tests, doc comment). |
| `place.rs` | `TARGET_ROWS` 7 → 4; doc comment "3 px rows + 1 caption". |
| `sprite.rs` | guard `h <= 12` → `h <= 6` (message: "3-row budget"). |
| `sprites/sheep.sprite`, `sprites/goat.sprite` | redrawn `16 × 6`, all 5 states. |
| `README.md` | `strip_rows` default 7 → 4; example height. |
| snapshots | regenerated (shorter band). |

**Why 6 px / 4 rows.** Exactly halves the pet pixel band (12→6), the most
defensible literal reading of "roughly half"; the strip goes 7→4 rows (~57%),
plainly "roughly half." Keeps the dedicated caption row (Phase 2's hover-doesn't-
shift-herd invariant). A 16×6 animal is small but a well-drawn one stays legible
(pixel-art animals routinely read at this size); the redraw preserves the
silhouette — ears/head, fluffy body, legs, one eye.

**Sprite redraw approach.** Faithful vertical compression of the *current*
sheep/goat look into 6 rows (not the new art — that's Improvement 3). Each of the
five state blocks keeps its existing per-state variation (idle/working leg
shuffle, etc.) at the new height. Roles (`#`/`L`/`M`/`S`/`p`/`e`) and per-state
headers (`frame_ms`, `motion`, `overlay`) are unchanged — only the grids shrink.

**No behavioral coupling.** `place`/controller ratios are computed from
`target_rows` at runtime, so lowering the constants Just Works. `slim_ratio`
already clamps a tiny target sanely.

## 5. Error handling

No new fallible paths. Malformed config still degrades to defaults (now
`strip_rows = 4`). A sprite that violates the new guard fails the test loudly
(intended).

## 6. Testing (TDD — failing test first)

1. **Guard first.** Tighten `every_embedded_species_is_valid` to `h <= 6`. With
   the still-12px sprites this **fails** → then shrink the sprites to green it.
   (Red-before-green proves the guard bites.)
2. `config`: update `default_config_has_the_opinionated_values` and
   `from_toml_str_defaults_missing_keys_and_ignores_comments` to expect
   `strip_rows == 4`. Add nothing new — the parser is unchanged.
3. `place`: `slim_ratio` tests already pass literal targets; add/adjust a test
   pinning `TARGET_ROWS == 4`.
4. `render`: `PET_PX_H` isn't asserted directly; the four snapshot tests are the
   regression net. Regenerate and eyeball each (pets legible, band ~3 rows,
   caption row intact, overflow `+N` still placed).
5. Full gate green.

## 7. Verification

- Snapshots reviewed by eye: each state's pet is recognisable at 6 px; the
  `renders_each_state_in_the_strip` snapshot shows all five with overlays; the
  caption snapshot still shows the label on its own bottom row; overflow `+N`
  still renders.
- `cargo run -- place` height sanity is covered by the `slim_ratio`/`TARGET_ROWS`
  unit tests (live placement unchanged from Phase 2/3, only shorter).

## 8. Guardrails

- Branch `feat/pets-shorter-strip` off the Phase-4 stack tip (see decisions.md);
  never commit to `main`.
- Keep it to the vertical footprint — no art redesign, no occlusion work, no
  width/capacity changes. Those are later improvements.
- Gate green before done.
