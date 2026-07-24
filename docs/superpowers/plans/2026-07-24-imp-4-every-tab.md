# Improvement 4 — Pets in every tab (plan)

Spec: [`2026-07-24-imp-4-every-tab-design.md`](../specs/2026-07-24-imp-4-every-tab-design.md)

**Execution note.** A focused refactor of one module (`control.rs`) plus a new
pure geometry parser. Done directly with TDD; the pure parser and the sweep
composition are unit-tested over the existing `HerdrCli` fake.

## Tasks (red → green each)

1. **`find_bottom_strip_target` (pure, red first).** Failing tests: single-pane
   layout → that pane + rows; top+full-width-bottom → the bottom pane; left|right
   columned → `None`; malformed → `None`. Implement (Value-navigation of `area` +
   `panes[].rect`).

2. **Widen `plan_injections`.** Drop the `pane_count == 1` gate — return every
   strip-less tab paired with one of its pane ids. Update its test.

3. **`inject_strip` takes a target pane + its rows.** Split that pane at
   `slim_ratio(pane_rows, target_rows)`, run, rename; no internal layout fetch.
   Update the ordering test.

4. **`sweep_once` uses geometry.** Per candidate: fetch layout →
   `find_bottom_strip_target` → inject or skip. Update the sweep tests (inject a
   single-pane and a top+bottom multi-pane tab; skip a columned one; isolate one
   failing tab).

5. **Verify against captured live layouts** (single-pane, top+bottom, columned);
   one live inject into a scratch top+bottom tab (marker survives).

6. **Docs:** decisions.md (design + the auto-start limit), HANDOFF.md.

7. **Gate + commit + PR** off `feat/pets-every-tab`.
