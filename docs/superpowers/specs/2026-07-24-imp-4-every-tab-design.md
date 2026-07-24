# Improvement 4 — Pets in every tab & workspace (design)

**Date:** 2026-07-24
**Kind:** post-roadmap improvement
**Status:** approved (autonomous run — brainstorming gate waived), pre-implementation
**Resolves against:** [`GOAL.md`](../../../GOAL.md) — "Always visible… wherever you are"
**AND** its hard constraint "Injection must never disturb running work."

## 1. Problem & goal

The controller auto-injects a strip into every **single-pane** tab (all new tabs)
and respawns closed ones — but it **skips every multi-pane tab** (`needs_strip`
requires `pane_count == 1`), because the only full-width primitive,
`layout.apply`, **kills the process in every pane it rebuilds** (verified Phase 3
spike — the non-negotiable constraint). So a pre-existing multi-pane tab never
gets a pet strip automatically.

**Goal:** the strip is present in **every tab, across every workspace** — new and
existing, single- and multi-pane — **without ever killing running work**.

## 2. The mechanism (verified live, herdr 0.7.0, 2026-07-24)

`herdr pane layout --pane <p>` returns the whole **tab** layout: the tab `area`
`{x,y,width,height}` and every pane's `rect`. That is enough geometry to find,
non-destructively, a **full-width bottom pane** — a pane whose `rect` spans the
tab width (`rect.x == area.x && rect.width == area.width`) and touches the tab's
bottom edge (`rect.y + rect.height == area.y + area.height`).

- A **single-pane** tab: its sole pane *is* the full-width bottom pane.
- A very common **multi-pane** shape — content on top, a full-width pane
  (terminal/agent) across the bottom — *has* a full-width bottom pane.
- Splitting that pane `down` (`pane split`, non-destructive — the pane's process
  survives) yields a **full-width** strip beneath it. No `layout.apply`, no kill.

Only tabs whose **bottom edge is split into side-by-side columns** have no
full-width bottom pane; a full-width strip there is impossible without the
destructive rebuild, so they stay on the on-demand `place` (the existing GOAL
compromise). This is the honest, bounded limit of "never kill work."

`tab list` is **session-wide** (all workspaces), so sweeping it already covers
every workspace — the "every workspace" ask needs no extra work.

## 3. Design

Widen auto-injection from "single-pane tabs" to "**any tab with a full-width
bottom pane**", split that pane, size the strip to the *pane's* rows.

| Unit | Change |
|---|---|
| `control.rs` `StripTarget` (new) | `{ pane_id, pane_rows }` — the pane to split + its row count. |
| `find_bottom_strip_target(layout_json)` (new) | Parse `area` + `panes[].rect`; return the full-width, bottom-aligned pane (bottom-most on ties), else `None`. Tolerant (Value-navigation). |
| `plan_injections(tabs, panes)` | Drop the `pane_count == 1` gate: return **every** tab without a strip, paired with any one of its pane ids (used only to fetch that tab's layout). |
| `sweep_once` | For each candidate: fetch its layout, `find_bottom_strip_target`; if `Some`, inject; if `None` (columned bottom), skip (leave to `place`). Per-tab failure still logged and isolated. |
| `inject_strip(cli, target_pane, pane_rows, self_exe, target_rows)` | Split `target_pane` `down` at `slim_ratio(pane_rows, target_rows)` (ratio now relative to the **pane** being split, not the whole tab — so the strip is ~`target_rows` regardless of where the pane sits), run the renderer, stamp the de-dup label. Drops its own `pane layout` fetch (done in the sweep). |

De-dup by label (`tabs_with_strip`) is unchanged, so a tab already holding a
strip — including the many single-pane tabs the controller already covers — is
never given a second one, and re-sweeps never stack strips.

**Unobtrusiveness.** Splitting a full-width bottom pane shrinks it by the strip
height (as single-pane injection already does) but never kills it — consistent
with the existing behavior and the GOAL principle. Columned-bottom tabs are left
untouched (no surprise rearrangement of a complex layout).

**Out of scope (documented limit):** auto-*starting* the controller on a fresh
herdr session still needs a plugin-start hook herdr doesn't fire (Phase 0 Spike
B); the user starts it once via `start-pets-controller` / `herdr-pets control`.
Recorded in decisions.md + HANDOFF.md.

## 4. Error handling

`find_bottom_strip_target` is tolerant: malformed or absent geometry ⇒ `None` ⇒
the tab is skipped, never a crash. Per-tab inject failure is logged and skipped
(sweep continues) — unchanged.

## 5. Testing (TDD — failing test first)

- `find_bottom_strip_target`: (a) single-pane layout → that pane, its rows; (b)
  top + full-width-bottom two-pane layout → the bottom pane; (c) left|right
  columned layout → `None`; (d) a tab already holding a full-width strip → still
  finds a full-width bottom pane (de-dup happens earlier, by label). Malformed →
  `None`.
- `plan_injections`: now returns multi-pane tabs too (still skips
  already-stripped tabs); pairs each with a pane in that tab.
- `inject_strip`: updated call sequence — split the given target pane at the
  pane-relative ratio, run, rename (no self-fetch of layout).
- `sweep_once`: injects into a single-pane tab **and** a top+bottom multi-pane
  tab, but **not** a columned-bottom tab; one failing tab doesn't abort the rest.

## 6. Verification

- Unit tests above (pure geometry + sweep composition over the CLI seam).
- Live: NOT unleashing the unbounded controller on the maintainer's ~40-tab
  session (same restraint as Phase 3, decisions.md). Verify `find_bottom_strip_
  target` against **real captured** `pane layout` JSON from a single-pane, a
  top+bottom, and a columned tab; and inject once into a single scratch
  multi-pane (top+bottom) tab to confirm a full-width strip lands
  non-destructively (marker process survives).

## 7. Guardrails

- Branch `feat/pets-every-tab` off the Improvement-3 tip (stacked); never commit
  to `main`.
- Never use `layout.apply` for auto-injection (the hard constraint). Full-width
  only via non-destructive `pane split` of a full-width bottom pane.
- Gate green before done.
