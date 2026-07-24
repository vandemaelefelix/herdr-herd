# Improvement 3 — New sprites + never-occluded icons (plan)

Spec: [`2026-07-24-imp-3-new-sprites-design.md`](../specs/2026-07-24-imp-3-new-sprites-design.md)

**Execution note.** Art + a small render-layout change — needs first-hand visual
judgment (and a published Artifact for the maintainer's phone review), so it is
done directly with TDD, not dispatched to subagents.

## Tasks (red → green each)

1. **Non-occlusion (render, red first).** Add `overlays_never_occlude_the_pet`
   and `overflow_counter_lives_in_the_top_lane` — assert the icon/`+N` sit on
   row 0 with no pet pixels there, and the pet is in the band below. They fail
   against the old on-pet layout.

2. **Reserve the lane (green).** In `draw_herd`, draw the pet band into a
   sub-rect (`y+1`, `height-2`) below a reserved top lane; move overlays + `+N`
   into the lane (row 0). Regenerate + review the four render snapshots.

3. **Strip height.** `config.strip_rows` 4→5, `place::TARGET_ROWS` 4→5 (1 lane +
   3 pixel rows + 1 caption), with their pins.

4. **New art.** Redraw `sheep` (16×6) + `goat` (13×6) to the artifact's
   side-view look (top-lit `L→M→S` wool, `#`-framed head with eye `e` + peach
   snout `ppp`, visible `p` legs, goat horns `hh` + beard). Headers unchanged, so
   motion/overlay behaviour is preserved. Sprite guard (`h<=6`) stays green.
   NB: legs use `p` (skin), not `#` (outline) — `#` is near-invisible on the
   dark theme the app ships with.

5. **Review Artifact.** Publish a contact sheet (all states × species, a crowded
   strip with icons + `+N`, a hue-identity row) rendered through the engine
   palette; iterate in place (same URL) on the maintainer's feedback.

6. **Gate + commit + PR** off `feat/pets-new-sprites`.
