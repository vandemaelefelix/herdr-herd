# Improvement 1 — Shorter strip (plan)

Spec: [`2026-07-24-imp-1-shorter-strip-design.md`](../specs/2026-07-24-imp-1-shorter-strip-design.md)

**Execution note.** This is a single, tightly-coupled change (drop three height
constants + redraw two sprites to the new budget). It is not decomposable into
independent parallel tasks, so it is implemented directly with TDD rather than
dispatched to subagents (subagent-driven-development is for independent tasks;
pixel-art redraw needs first-hand visual judgment on the snapshots). Later
improvements with genuinely independent work use subagents.

## Tasks (in order, red → green each)

1. **Tighten the height guard (red).** In `sprite.rs`, change the
   `every_embedded_species_is_valid` assertion `h <= 12` → `h <= 6` (msg
   "taller than the 3-row budget"). Run tests → this **fails** on the current
   12-px sprites. Proves the guard bites before the fix.

2. **Redraw the sprites (green the guard).** Rewrite `sprites/sheep.sprite` and
   `sprites/goat.sprite` to `16 × 6`, all five state blocks, headers unchanged.
   Preserve silhouette + per-state variation. Re-run → guard green.

3. **Drop the render constant.** `render.rs`: `PET_PX_H` 12 → 6.

4. **Drop the config default.** `config.rs`: `strip_rows` default 7 → 4; update
   the two unit tests that pin the default; update the doc comment.

5. **Drop the place constant.** `place.rs`: `TARGET_ROWS` 7 → 4; doc comment;
   add/adjust a test asserting `TARGET_ROWS == 4`.

6. **Regenerate + review snapshots.** `cargo test`; `cargo insta accept` (or
   review the `.snap` diffs) for the four render snapshots. Eyeball: pets
   legible, ~3-row band, caption row intact, `+N` present.

7. **Docs.** `README.md`: config table `strip_rows` default 7 → 4; example.

8. **Gate + commit.** `cargo test && cargo clippy --all-targets -- -D warnings
   && cargo fmt --check`; commit; open PR `feat/pets-shorter-strip`.
