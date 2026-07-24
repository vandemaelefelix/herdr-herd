# Handoff — 2026-07-24 (session 2: post-roadmap improvements + sizing iteration)

**Read this first**, then [`docs/HANDOFF.md`](HANDOFF.md) (phase history) and
[`docs/decisions.md`](decisions.md) (every judgment call, most recent at the
bottom). This doc captures the state at a context-clear during a live back-and-
forth with the maintainer over the pet strip's **sprites and sizing**.

## TL;DR of where we are

The five roadmap phases were already Done. This session added **four maintainer-
requested improvements** on top, as a stacked set of open PRs, and then iterated
**twice more** on the sprite look/size from live feedback. Everything is on the
tip branch **`feat/pets-every-tab`**; the gate is green (104 tests, clippy, fmt).
Nothing is merged (the harness blocks autonomous merges to `main`).

**There is one open issue the maintainer flagged — see "⚠️ THE SPRITE ISSUE"
below. Resolve that next.**

## The gate (must stay green)

```
cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check
```

## The PR stack (bottom → top; base = the branch below)

| PR | Branch | What |
|---|---|---|
| #3 | `feature/phase-2` | Phase 2 (interactivity/placement) |
| #4 | `feat/phase-3-controller` | Phase 3 (controller) |
| #5 | `feat/phase-4-config-polish` | Phase 4 (config/polish) |
| #6 | `feat/pets-shorter-strip` | Imp 1 — halve the strip |
| #7 | `feat/pets-hover-label` | Imp 2 — hover shows `workspace › tab` breadcrumb, not "claude" |
| #8 | `feat/pets-new-sprites` | Imp 3 — new sprites + reserved icon lane (**iterated 3×** — see below) |
| #9 | `feat/pets-every-tab` | Imp 4 — inject into every tab with a full-width bottom pane. **This is the tip / current branch — contains everything.** |

Merge order when ready: #3→#4→#5→#6→#7→#8→#9. Branching off the stack tip (not
`main`) is deliberate — see decisions.md "Branch these improvements off the
Phase-4 stack tip".

## ⚠️ THE SPRITE ISSUE (maintainer-flagged — top priority)

The sprites have been through **three versions**, and the current one is **NOT
the maintainer's "correct" (artifact) sprites** — I replaced them by mistake when
slimming down. The saga:

1. **v1** (commit `2f0dcea`): I *hand-drew* a compact 16×6 sheep with a single
   standing pose. Maintainer: "you didn't use the ones from the artifact that
   already existed" (Artifact `85ac4f4a` — the sprite-playground, traced from
   `sheep_assets_x4`, with real per-state animation).
2. **v2** (commit `2dc1378`): I copied the **artifact's actual frame grids**
   verbatim (lying-down idle `row4_f0`, two-frame walk `row1_f0/f1`, standing),
   normalised to **16×14**. `PET_PX_H=14`, strip 9 rows. **These are the
   "correct" artifact sprites the maintainer approved.**
3. **v3** (commits `cb463f5` + `4a94d7b`, current): maintainer said the sheep
   clipped the bottom and the strip was too tall → I **hand-drew new small
   12×5 sprites** and shrank the band (`PET_PX_H=6`). **In doing so I discarded
   the artifact-faithful v2 art instead of scaling it down.**

**So `sprites/sheep.sprite` + `sprites/goat.sprite` right now are my hand-drawn
12×5 versions, not the artifact's traced art.** That is the discrepancy the
maintainer noticed.

**Recovery pointer:** the artifact-faithful 16×14 sprites are intact in git:
```
git show 2dc1378:sprites/sheep.sprite
git show 2dc1378:sprites/goat.sprite
```
The raw source is Artifact `85ac4f4a` (frames `row1_f0/f1`, `row2_*`, `row4_f0`).

**The unresolved design tension to settle with the maintainer:**
- They want the artifact's *actual* sheep art (v2), AND a *small/short* strip.
- v2's frames are ~13–14 px tall; the slim strip wants a ~5–6 px band.
- v3 resolved it by hand-drawing small (losing the artifact art). The maintainer
  wants the **artifact art, scaled/adapted down** — i.e. keep the artifact's
  silhouette/poses but at the small size. That redraw hasn't been done.
- **Also note the herdr height constraint below** — it caps how short the strip
  can actually get regardless of sprite size, so "small sprite" and "short
  pane" are somewhat independent problems.

## ⚠️ The herdr minimum-pane-height constraint (the real blocker on "shorter strip")

Verified live (herdr 0.7.0): **herdr clamps any new pane to ~10% of the tab
height minimum** — ≈9 rows on the maintainer's 86-row display (≈7 on a 64-row
tab). Probed `pane split` at ratios 0.90–0.99; all clamp to ~8–9 rows. **No
herdr config lowers it** (`~/.config/herdr/config.toml` has only
`sidebar_min_width`). So the plugin **cannot make the strip pane shorter than
herdr's minimum**; a `strip_rows`/`TARGET_ROWS` below it is silently clamped up.

Mitigation shipped in v3: `draw_herd` **bottom-aligns** the strip (caption on the
bottom row, pet band above it, icon lane above that), so the content reads slim
and the unavoidable extra rows fall at the top and blend with the pane above.
On a normal ~40–50-row terminal the strip is naturally ~4–5 rows; only on the
maintainer's very tall display does herdr's 10% floor make it ~9.

**Open question posed to the maintainer (unanswered at clear):** given herdr caps
the pane height, do they want (a) leave as-is (slim content, herdr floor pane —
my lean); (b) fill the ~9 rows with slightly larger pets; or (c) treat it as a
herdr limitation to raise upstream?

## Current sizing knobs (v3)

| Where | Value | Meaning |
|---|---|---|
| `render.rs` `PET_PX_H` | `6` | pet band = 3 half-block rows |
| `config.rs` `strip_rows` default | `5` | 1 icon lane + 3 band + 1 caption (clamped up by herdr) |
| `place.rs` `TARGET_ROWS` | `5` | same |
| `sprite.rs` guard | `h <= 5` | sprite must be ≤5 px (1 px headroom in the 6 px band) so the hop never clips |
| `anim.rs` Hop/Shake dy | capped at `1` px | fits the 1 px headroom |
| `render.rs` `draw_herd` | **bottom-anchored** pets + **bottom-aligned** strip | feet on the band floor; content hugs the pane bottom |

Sprites are `12×5` (sheep + goat, goat has a horn). Idle = lying doze, working =
2-frame walk, done/blocked/unknown = standing.

## What works (verified)

- **Imp 2 hover** confirmed live: the demo strip caption showed
  `Home folder › Herdr remote control` (workspace › tab), not "claude".
- **No clipping**: bottom-anchor + `h≤5` guard + 1 px hop cap; verified via
  throwaway renders.
- **Non-occlusion**: icons live in the reserved lane, never on a pet (tests +
  snapshots).
- **Imp 4 geometry** validated against 16 real live tab layouts (15 injectable,
  1 columned-bottom correctly skipped).

## Live runtime state at clear

- **Old controller STOPPED**: I `pkill`ed the pre-changes controller (was
  pid 61144) — it had been racing me, auto-injecting *old* 7-row strips into any
  new tab. No controller is running now.
- **~34 old 7-row strips** from that old controller still exist across tabs
  (their `render` processes keep running until each pane is closed). Not cleaned
  up — the maintainer can close them by the `herdr-pets` pane label, or they
  vanish on session restart.
- **`pets-demo` tab** (`w1Y:tT`, currently focused): a top pane running `git log`
  + a strip pane (`w1Y:p28`) running the **fresh** binary — the current v3 12×5
  bottom-aligned sheep, 9 rows (herdr floor). This is what the maintainer is
  looking at.
- **`pets-controller` tab** (`w1Y:tJ`): still present from earlier; held the old
  controller + its strip.
- The release binary at `target/release/herdr-pets` is **freshly built from the
  tip** (has v3). NOTE: an earlier `cargo build --release` was a silent no-op
  (stale binary) — if in doubt, `touch src/*.rs && cargo build --release`.

## Review artifact (⚠️ stale)

`https://claude.ai/code/artifact/fac9781a-d0d5-4d04-b0f1-8aaf7aa92f17` — the
phone-review contact sheet. **It currently shows the v2 16×14 animated sprites**
(last republished at v2); it was NOT updated for v3's 12×5. So it happens to show
the artifact-faithful art, not what's in the repo now. Re-publish it (same
file path in scratchpad: `.../scratchpad/sprite-review.html`, same URL) once the
sprite question is settled.

## Suggested next steps for the new agent

1. **Settle the sprite question with the maintainer** (do NOT guess): they want
   the artifact's actual sheep (v2 / Artifact `85ac4f4a`), and it was replaced
   by a hand-drawn 12×5 in v3. Options: (a) faithfully trace the artifact sheep
   down to the small band (best — keeps the art, fits the slim strip); (b) go
   back to v2's 16×14 art and accept the taller strip; (c) keep v3. Recover v2
   via `git show 2dc1378:sprites/*.sprite`.
2. **Confirm the strip-height decision** given herdr's ~10% min-pane floor
   (the open (a)/(b)/(c) question above).
3. Re-publish the review artifact to match whatever sprites land.
4. Keep the gate green; update `docs/decisions.md` with any new calls.
5. PRs are all open on GitHub (`gh pr list`); merge order above.

## Anchor docs

- [`docs/HANDOFF.md`](HANDOFF.md) — phase + improvement overview.
- [`docs/decisions.md`](decisions.md) — full judgment-call log (Improvement 3 has
  three revision entries documenting the sprite saga).
- [`GOAL.md`](../GOAL.md) — north star (incl. the "never disturb running work"
  injection constraint that shapes Imp 4).
- Specs/plans: `docs/superpowers/specs/2026-07-24-imp-{1,2,3,4}-*` and plans.
