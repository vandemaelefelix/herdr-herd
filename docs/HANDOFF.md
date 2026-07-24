# herdr-pets — Handoff / Status

**As of:** 2026-07-24 · **Written for:** resuming after a context clear.

## TL;DR

All five phases (0–4) in [`docs/PLAN.md`](PLAN.md) are **Done**, and a second
autonomous run has since delivered **four maintainer-requested improvements**
(shorter strip, richer hover label, new sprites + never-occluded icons, pets in
every tab) as a further stacked set of open, green PRs. Nothing is merged yet
(the harness blocks autonomous merges to `main`).

**The single most important next action: merge the whole PR stack in order —
phases #3 → #4 → #5, then improvements #6 → #7 → #8 → #9.**

## Post-roadmap improvements (2026-07-24) — the newest work

Four improvements, each its own stacked PR (base = the branch below it), built
after the phases. Specs in `docs/superpowers/specs/2026-07-24-*`, plans
alongside, judgment calls in [`docs/decisions.md`](decisions.md) under
"Post-roadmap improvements".

| PR | Branch | Base | What |
|---|---|---|---|
| [#6](https://github.com/vandemaelefelix/herdr-pets/pull/6) | `feat/pets-shorter-strip` | `feat/phase-4-config-polish` | Halve the strip: pet band 12→6 px, strip 7→4 rows; sheep/goat recompressed. |
| [#7](https://github.com/vandemaelefelix/herdr-pets/pull/7) | `feat/pets-hover-label` | `feat/pets-shorter-strip` | Hover shows the herdr sidebar breadcrumb `workspace › tab`, not `claude` (join `workspace list`/`tab list` in the watcher). |
| [#8](https://github.com/vandemaelefelix/herdr-pets/pull/8) | `feat/pets-new-sprites` | `feat/pets-hover-label` | The artifact's **animated** sheep/goat (lying idle, walk-cycle working, standing others) + a reserved icon lane so overlays/`+N` never cover a pet. Redrawn **small** (12×5) and **bottom-anchored** so a hop never clips; strip = **5 rows** (slim status line). Review Artifact: https://claude.ai/code/artifact/fac9781a-d0d5-4d04-b0f1-8aaf7aa92f17 |
| [#9](https://github.com/vandemaelefelix/herdr-pets/pull/9) | `feat/pets-every-tab` | `feat/pets-new-sprites` | Controller injects into every tab with a full-width bottom pane (single-pane **and** top+bottom multi-pane), non-destructively — not just single-pane tabs. |

Current working branch: `feat/pets-every-tab` (tip of the whole stack — contains
everything). Net user-visible strip: **5 rows** (1 icon lane + 3-px-row pet band
+ 1 caption); `config.strip_rows` default is now **5**. Pets are small (12×5) and
bottom-anchored so a hop never clips — a deliberately slim status line (see
decisions.md "Improvement 3 (revision 2)").

## Phase stack (below the improvements)

The phases were built as a **stack** because this environment blocks merging to
`main` autonomously (see [`docs/decisions.md`](decisions.md), "Stacked phase
branches"). Each PR's base is the branch below it; the improvement PRs above sit
on top of `feat/phase-4-config-polish`:

| PR | Branch | Base | Tip | Phase |
|---|---|---|---|---|
| [#3](https://github.com/vandemaelefelix/herdr-pets/pull/3) | `feature/phase-2` | `main` | `68edc30` | 2 — Interactivity & placement |
| [#4](https://github.com/vandemaelefelix/herdr-pets/pull/4) | `feat/phase-3-controller` | `feature/phase-2` | `47deb50` | 3 — Controller / watchdog |
| [#5](https://github.com/vandemaelefelix/herdr-pets/pull/5) | `feat/phase-4-config-polish` | `feat/phase-3-controller` | `5b1a26b` | 4 — Config & polish |

**Merge order:** #3 → #4 → #5, then the improvements #6 → #7 → #8 → #9. As each
lands, GitHub auto-retargets the next PR's base to `main` (or retarget manually).
All are green (the gate passes locally — see below). `main` currently has only
Phases 0–1.

The current working branch is `feat/pets-every-tab` — the tip of the full stack
(phases + all four improvements).

## Phase status (all Done)

| Phase | Delivered | Design / Plan |
|---|---|---|
| 0 | Plugin scaffold, socket client, spikes A/B | specs/plans `…phase-0-foundations…` |
| 1 | Deterministic identity, half-block sprites, animations, live updates | `…phase-1-renderer-core…` |
| 2 | `place` subcommand + `place-pets` action (full-width strip via `layout.apply`); mouse hover→caption, click→`agent focus`; `Pet.label`; `socket::request_line` | `…phase-2-interactivity-placement…` |
| 3 | `control` watchdog: poll-sweep, single-owner lock (`src/lock.rs`), label de-dup, **non-destructive** `pane split` injection; `start-pets-controller` action | `…phase-3-controller…` |
| 4 | `src/config.rs` (4 knobs, hand-rolled tolerant parse, no new deps); wired into control/render; reduced-motion; README; CI workflow | `…phase-4-config-polish…` |

Specs live in `docs/superpowers/specs/`, plans in `docs/superpowers/plans/`
(all dated `2026-07-23`).

## ⚠️ Live runtime state right now

A **controller is running** (started during the live test at the user's request):
- Process: `herdr-pets control`, **pid 61144** (dev binary from this worktree).
- Hosted in a dedicated herdr tab labelled **`pets-controller`**.
- It has injected a pet strip into **~34 single-pane tabs** across all workspaces
  and keeps them there (respawns closed ones, strips new tabs).

**To stop it:** go to the `pets-controller` tab and `ctrl+c`, or
`pkill -f "target/release/herdr-pets control"`. **To remove the strips
afterward:** close every pane labelled `herdr-pets` (they auto-close when their
`render` process dies, so killing the controller + the renders clears them; or
close by label). It will **not** auto-restart on a fresh herdr session (no
plugin-start hook — Phase 0 Spike B).

There is also a `pets-demo` tab with an on-demand `place` strip (from the test).

## Key decisions & findings

Full log: [`docs/decisions.md`](decisions.md). The load-bearing ones:

1. **The CI gate was red on merged Phase 0/1 code** (never run through
   `cargo fmt`/`clippy`). Fixed as two prerequisite commits at the start of PR #3.
2. **`layout.apply` KILLS the process in every pane it rebuilds** (verified live).
   So the controller must **never** use it for auto-injection — it uses
   non-destructive `pane split`, which is full-width **only on single-pane tabs**.
   This refined GOAL.md's "always everywhere" compromise: auto-injection covers
   single-pane + new tabs; pre-existing **multi-pane** tabs stay on on-demand
   `place`. (GOAL.md "Injection must never disturb running work".)
3. **reduced_motion** must freeze BOTH the roam step and the animation-phase
   advance (`simulate_tick`) — a whole-branch review caught that the first cut
   only froze horizontal wander (pets kept bouncing).

## What's left / next actions

1. **Merge the stack** #3 → #4 → #5, then #6 → #7 → #8 → #9 (the only thing
   between here and "shipped"). Review the sprite Artifact (linked above) first.
2. **Stop the controller** if you don't want it running (see above).
3. **Deferred (documented, not built)** — pick up if/when wanted:
   - `scope` config knob (limit the controller to a workspace/tab instead of the
     whole session) — the test showed why this matters.
   - A **plugin-start / auto-launch** story so the controller survives session
     restarts without a manual relaunch.
   - Packaging: prebuilt release binaries / `fetch-or-build`, and cutting a
     tagged GitHub release (`herdr plugin install <owner>/<repo>` already builds
     from source via the manifest `[[build]]`).
   - Palette customization, per-state behavior overrides.
   - **Kitty-graphics sprites** (roadmap stretch; half-block stays the default).

## How to resume / verify

- **Rebuild + relink:** `cargo build --release -p herdr-pets && herdr plugin link .`
- **The gate (must stay green):**
  `cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check`
  — currently **92 tests pass**, clippy/fmt clean on `feat/phase-4-config-polish` (`5b1a26b`).
- **Try it:** `herdr-pets place` (from inside a target tab's pane) for a one-shot
  strip; `herdr-pets control` for the watchdog; config at
  `$(herdr plugin config-dir herdr-pets)/config.toml` (keys: `enabled`,
  `strip_rows`, `sweep_interval_ms`, `reduced_motion`).
- **Rust skills / conventions:** see `CLAUDE.md` and `.claude/skills/`.
- **SDD progress ledger** (git-ignored scratch): `.superpowers/sdd/progress.md`
  has the per-task commit trail if you need it.

## Anchor docs

- [`GOAL.md`](../GOAL.md) — north star + locked decisions (incl. the injection constraint).
- [`docs/PLAN.md`](PLAN.md) — phase roadmap + tracker (all Done).
- [`docs/decisions.md`](decisions.md) — every judgment call from the autonomous run.
