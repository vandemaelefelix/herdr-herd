# Improvement 3 — New sprite design + never-occluded icons (design)

**Date:** 2026-07-24
**Kind:** post-roadmap improvement
**Status:** approved (autonomous run — brainstorming gate waived), pre-implementation
**Resolves against:** [`GOAL.md`](../../../GOAL.md) — "Glanceable… recognizable identity…
Useful, not merely cute."

## 1. Goal & exit criteria

Two things:

1. **New sprite look.** Adopt the maintainer's updated sheep design (Claude
   Artifact `85ac4f4a` — a clean side-view sheep: woolly `M`/`S` coat, a peach
   snout `p` + dark eye `e` head on the right, four legs) into `sprites/*.sprite`,
   within the halved 6-px band from Improvement 1. Goat updated to match the same
   flatter aesthetic.
2. **Icons never occlude the pet.** State overlays (`Zz`/`!`/`?`) and the `+N`
   overflow marker must **never** cover any pet pixel, in every state and at the
   crowded/overflow density. Reserve space for them.

**Exit criteria:**
- Overlays + `+N` render in a **dedicated top lane**; the pet band sits below it;
  the full pet is visible in all five states and when the strip overflows.
- `sheep` + `goat` redrawn to the new look (16×6 / 13×6), all states, gate green.
- A published **Artifact** shows all states + a crowded strip (icons + pets
  together) for phone review; iterated in place (same file/URL).
- Gate green.

## 2. The tension, resolved

Improvement 1 halved the strip (pet band 12→6 px; strip 7→4 rows, caption at the
bottom, overlays still drawn *on* the pet's top row — same as the original, which
already occluded). "Never occlude" needs the badge to have somewhere to go that
isn't the pet. Half-block rows are atomic (2 px), and at crowded density there is
no horizontal gap between pets, so only a **reserved vertical lane** guarantees
non-occlusion regardless of density.

**Decision:** keep the pet **pixel band halved** (6 px = 3 rows — Improvement 1's
core, the legibility floor) and add **one thin badge lane** on top. Strip layout
becomes 5 rows:

```
row 0            badge / bubble / +N lane   (reserved — no pet pixels)
rows 1..3        pet pixel band (PET_PX_H = 6)
row 4 (bottom)   hover caption
```

Net vs original: 7 → 5 rows (still shorter), band 12 → 6 px (still halved), and
now **nothing ever covers a pet**. `config.strip_rows` default 4→5,
`place::TARGET_ROWS` 4→5.

## 3. Grounding facts

- `draw_herd` builds a `PixelBuf(strip_w, PET_PX_H)`, blits pets, `draw_pixels`
  emits it from `area.y`. Overlays are text spans at `area.y` (row 0, *on* the
  pet). `+N` is a span at `area.y + area.height/2` (mid-band, *on* a pet).
- Hover hit-testing (`pet_at_column`) is purely **horizontal** (column→pixel-x),
  so shifting the band down a row does not affect it. No mouse-path change.
- Caption is drawn by `run_loop` via `draw_caption` at `area.bottom()-1` — its
  own row already; unchanged.
- Render snapshot tests use the 4×4 `test-blob`, so they *will* shift (badge lane
  on row 0, blob down one row) — regenerate + review (expected).

## 4. Design

### 4.1 Render (`render.rs`)

- Reserve `badge_y = area.y` for overlays + `+N`.
- Draw the pet band into a sub-rect `pet_area = { x, y: area.y+1, width,
  height: area.height.saturating_sub(2) }` (excludes the badge lane on top and
  the caption row at the bottom). `draw_pixels` already clips to the rect, so a
  short pane degrades gracefully.
- Overlays: span at `badge_y`, column near the pet (clamped to width).
- `+N`: span at `badge_y`, right-aligned (in the reserved lane — never on a pet).

### 4.2 Sprites

New side-view look in the artifact's flatter palette (heavy `M`/`S` coat, `p`
snout, `e` eye, `#` outline; `L` light-wool used sparingly for a top highlight).
One pose per species (state is conveyed by overlay + motion, as today); `idle`
and `working` keep two frames (leg/wool shuffle), the rest one. Headers
(`frame_ms`, `motion`, `overlay`) unchanged from the current files, so behavior
(breathe/hop/shake/sway, `Zz`/`!`/`?`, ghost) is preserved — only the art and the
lane change. Sizes stay `sheep 16×6`, `goat 13×6` (guard `h<=6` holds; widths
keep capacity/hit-test math stable).

### 4.3 Review artifact

An HTML Artifact rendering the **actual** new sprite grids through the engine's
role→color palette and the badge-lane layout: a panel per state (idle/working/
done/blocked/unknown) for sheep + goat, plus a **crowded strip** showing many
pets with their icons present, to prove no overlap. Published private, iterated
in place (same file path/URL) — Felix reviews on his phone.

## 5. Error handling

No new fallible paths. Short-pane degradation is graceful (clip, never panic).

## 6. Testing (TDD — failing test first)

- **Non-occlusion test (write first, fails):** render a single `done` pet (badge
  `!`) on a small backend; assert the badge glyph is on `area.y` (row 0) **and**
  the pet band rows (`y >= 1`) contain pet pixels at the badge's column — i.e.
  the badge and the pet occupy different rows, so the pet is not overwritten.
- **Overflow test:** `+N` renders on row 0 (badge lane), not mid-band.
- Regenerate the four render snapshots; review each (badge lane on top, band
  below, caption row intact).
- `config`/`place`: update the `strip_rows`/`TARGET_ROWS` pins (5).
- Sprite guard (`h<=6`) still green for the redrawn art.

## 7. Verification

- Snapshots reviewed by eye.
- Published Artifact reviewed (all states + crowded strip). Iterate the sprites
  until the pet reads well and no icon overlaps, redeploying the same Artifact.

## 8. Guardrails

- Branch `feat/pets-new-sprites` off the Improvement-2 tip (stacked); never
  commit to `main`.
- Keep behavior (motions/overlays/ghost) unchanged — this is art + layout only.
- Gate green before done.
