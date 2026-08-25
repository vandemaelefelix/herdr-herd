# Decision log

A running log of judgment calls made during the autonomous multi-phase run
(Phases 2–4). Each entry records a decision that would otherwise have been a
question for the maintainer, with the options weighed and the rationale — tied
back to [`GOAL.md`](../GOAL.md) where relevant.

---

## 2026-07-23 — Phase 2 — Model tiering for subagent-driven execution
**Question:** Which model tier to use for the per-task implementer and reviewer subagents.
**Options considered:** (a) most-capable everywhere; (b) cheapest everywhere; (c) mid-tier (Sonnet) for implementers/reviewers, most-capable (Opus) for the final whole-branch review.
**Decision:** Option (c). Sonnet for implementers and task reviewers; Opus for the final branch review.
**Rationale:** The Phase 2 plan ships complete code per task, so tasks are transcription + integration + testing — well within Sonnet. The final review needs the most judgment and guards the merge, so it gets the most capable model. This follows subagent-driven-development's Model Selection guidance while honoring the non-negotiable quality bar.

## 2026-07-23 — Phase 2 — Bring the crate to a green gate (pre-existing fmt + clippy debt)
**Question:** The quality bar requires `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check` to pass before a phase is done, but that gate is already **red on merged Phase 0/1 code** before Phase 2 touches anything: `cargo fmt --check` fails (49 hunks / 11 files — the code was never actually run through `cargo fmt`), and clippy `-D warnings` fails (4 `collapsible_if` errors in render.rs, sprite.rs, watcher.rs×2). How to honor the gate?
**Options considered:** (a) scope the gate to only Phase 2's new lines and leave the crate red — but then the gate never passes and the phase can't honestly be claimed done; (b) weaken/skip the gate — rejected, non-negotiable; (c) bring the whole crate to a green gate as prerequisite cleanup commits, then keep it green.
**Decision:** Option (c). Two clearly-labeled prerequisite commits on `feature/phase-2`: `refactor:` collapsing the nested ifs (via clippy's machine-verified `--fix`) and `style:` a one-time `cargo fmt` sweep. Every subsequent implementer runs `cargo fmt` before committing to keep it green.
**Rationale:** The gate is explicitly listed in the handoff as the definition of "done"; the only honest way to satisfy it is to make it pass. Both fixes are mechanical, behavior-preserving, and reviewable. This is prerequisite hygiene, not scope creep into Phase 3/4 features. Recorded here rather than silently worked around, per the handoff's contradiction-handling rule.

## 2026-07-23 — Phase 2 — Live mouse verification via synthetic PTY injection
**Question:** Task 8 §8.1 requires confirming herdr forwards mouse events into the plugin pane (hover → caption, click → focus). As an autonomous agent I cannot physically move a mouse or visually observe the pane. How to verify the gating interactivity without a human at the keyboard?
**Options considered:** (a) skip it and claim it works — rejected (dishonest, and the handoff makes mouse forwarding a gating check); (b) mark it unverified and stop — rejected (the run must not stop, and there is a testable path); (c) inject raw SGR mouse escape sequences into the strip pane's PTY via `herdr pane send-text`, which is the exact input channel crossterm reads, and observe effects programmatically.
**Decision:** Option (c). Injected a mouse-move at a strip column → read the pane's bottom row and saw an agent label (`claude`) rendered in the caption, proving PTY→crossterm→`pet_at_column`→`draw_caption`. Injected a left-click at a pet column → the focused pane jumped from my pane (`w1Y:t1`) to another workspace (`w7:tA`), proving click→`agent focus <terminal_id>`. Restored focus afterward.
**Rationale:** This exercises the entire in-process hover/click path inside a real herdr pane. The only micro-step not covered is herdr's own screen→pane-local mouse-coordinate translation before it writes to the PTY — standard multiplexer behavior, low-risk per Phase 0's spike framing ("panes are full PTYs"). The `place` layout mechanism itself was verified fully programmatically (root `down` split at ratio = slim_ratio(64,7) = 0.890625, existing tree preserved on top, a full-width 7-row bottom strip running the renderer), for both the `place` command and the `[[actions]]` invocation. No design assumption was contradicted, so no GOAL.md/PLAN.md change was needed beyond marking Phase 2 Done.

## 2026-07-23 — run-wide — Stacked phase branches (merging to main is blocked by the harness)
**Question:** Phases 3 and 4 build on Phase 2's code and the conventions say "always branch off main"; the mission says to loop each phase "after Phase 2 merges." How to proceed with no human available to merge?
**Options considered:** (a) merge each phase's PR to main myself once it passes the gate + whole-branch review, then branch the next phase off the updated main — my first choice; (b) leave PRs open and stack each next phase's branch on the previous phase's tip.
**Decision:** Option (b), forced by the environment. I attempted `gh pr merge 3` and the Claude Code auto-mode classifier **blocked** it (merging to main is not permitted autonomously in this harness). I did not work around the denial (a direct `git push` to main would bypass its intent and also violate the "never commit to main" convention). So: Phase 2 stays as open, green PR #3 (base `main`). Phase 3 branches off the `feature/phase-2` tip and opens a PR based on `feature/phase-2`; Phase 4 branches off Phase 3 similarly. The maintainer merges the stack (PR #3 first, then Phase 3, then Phase 4) when they return; each PR base can retarget to `main` as the one below it merges.
**Rationale:** The DoD explicitly accepts "a merged (or at least open, green) PR", so open green PRs satisfy it. Stacking keeps the phase dependency chain intact without stopping the run or bypassing a permission boundary. The only cost is that later PR diffs include earlier phases' commits until those merge — cosmetic and self-resolving.

## 2026-07-23 — Phase 3 — Auto-injection must be non-destructive (`layout.apply` kills processes)
**Question:** Phase 3's plan was to reuse Phase 2's `place` (built on `layout.apply`) to inject a full-width strip into every tab automatically. Is that safe for tabs with running agents?
**Options considered:** (a) reuse `layout.apply` everywhere as planned; (b) find a non-destructive primitive and scope auto-injection to where it works; (c) drop "always everywhere."
**Finding (live herdr 0.7.0 spike, 2026-07-23):** `layout.apply` **re-materialises every pane and KILLS its process** — a marker `sleep` (PID 25458) was SIGHUP-killed by an injection, confirming Spike A's hedged "processes may be disturbed" as a hard "processes are killed." By contrast, an incremental `pane split --direction down --ratio R` **preserves** the process (marker survived) and yields a full-width strip — **but only on a single-pane tab** (Spike A: on a multi-pane tab `split down` spans just one column). `plugin pane open --placement split` is also non-destructive and labels the pane "Pets" and runs the entrypoint, but forces a 50/50 ratio (not slim).
**Decision:** Option (b). Phase 3's controller injects **only non-destructively**, via `pane split down --ratio slim_ratio(rows,7)` + run the renderer, **scoped to single-pane tabs** (all new tabs are single-pane at creation, so "always everywhere" holds going forward). Pre-existing **multi-pane** tabs are NOT auto-rebuilt — they remain on the on-demand `place` (Phase 2), where the user accepts the rebuild. GOAL.md's compromise section and PLAN.md's Phase 3 were updated to state this constraint; this is a genuine north-star refinement, flagged (not silently worked around) per the handoff.
**Rationale:** GOAL.md's "unobtrusive / never interrupt work" is a hard principle; killing every agent to show a pet strip would be absurd. Non-destructive-only injection honors it while preserving the mission for the natural workflow. The honest limitation (pre-existing multi-pane tabs need on-demand placement) is a small, well-bounded gap.

## 2026-07-23 — Phase 3 — Verification scope: did NOT run the unbounded controller against the live session
**Question:** Phase 3's Task 6 calls for a live `control` run. But `control` sweeps EVERY tab across the whole herdr session and injects a strip into every eligible single-pane tab — in the maintainer's real ~46-pane, ~40-tab environment that is a wide, unsolicited change during an unattended autonomous run.
**Options considered:** (a) run the real unbounded controller and clean up its strips by label afterward; (b) verify the design-critical claims on an isolated scratch tab + a safe held-lock test, and rely on unit tests for the sweep/lock composition.
**Decision:** Option (b). I verified live, safely: (1) `inject_strip`'s exact command sequence on ONE scratch single-pane tab — a marker `sleep` (PID 84234) **survived** (non-destructive ✓), the strip landed full-width (w=214) and slim (h=7) at ratio 0.8906, was labeled `herdr-pets` (de-dup marker ✓), and rendered pets; (2) single-owner lock — with the lock held externally, the real `control` binary exited on its own with "another controller is already running" (code 0) and injected nothing. `sweep_once` eligibility, `plan_injections`, and lock acquire/release are covered by unit tests (83 tests pass). I did NOT unleash the unbounded loop on the shared session.
**Rationale:** The only risk that could contradict the design — "injection kills processes" — was fully verified false end-to-end. The remaining behavior (the poll loop stitching sweep+sleep) is thin glue over unit-tested parts. Running the real controller would strip dozens of the maintainer's live tabs for no additional correctness signal. Honoring "unobtrusive" applies to how I verify, too.

## 2026-07-23 — Phase 4 — Scope, config parser, and reduced-motion verification
**Question:** Phase 4 ("config & polish") is broad in the roadmap (config surface, palette/scope/per-state knobs, packaging/release, Kitty sprites). What to build, how to parse config without a new dependency, and how to verify reduced-motion in a live 46-agent session?
**Options considered / decisions:**
- **Scope:** shipped the four highest-value, opinionated knobs (`enabled`, `strip_rows`, `sweep_interval_ms`, `reduced_motion`) + README docs + CI gate. **Deferred** (recorded here + spec §2): prebuilt release artifacts / a cut GitHub release (the existing `[[build]]` source-build already makes `herdr plugin install <owner>/<repo>` work; tagging is a post-merge maintainer step, and the harness blocks autonomous merges), a `scope` filter, palette customization, per-state behavior overrides, and Kitty-graphics sprites (explicit roadmap *stretch*; half-block stays the universal default). Rationale: honors GOAL.md's "opinionated defaults, few knobs" and keeps the phase reviewable.
- **Config parser:** hand-rolled a tolerant `key = value` reader rather than adding `toml` as a runtime dep (`toml` is dev-only; the constraint forbids new runtime crates). Four flat scalar keys don't need full TOML.
- **reduced_motion verification:** a naive "diff two frame reads" live test was a **false negative** — with 46 live agents the strip churns from *state* changes (status glyphs/`Zz` toggling) and is packed edge-to-edge, so frame/position diffing can't isolate *wandering*. Verified instead by a complete causal chain: `config::load()` returns `reduced_motion=true` (proven by a throwaway live test), the flag is threaded `main → run → run_loop` (code-verified), `herd.step` is gated by it (code-verified), and `herd::reconcile` preserves existing pets' `x` so `step` is the *sole* mutator of their position — therefore skipping `step` provably freezes wandering. `enabled=false` was verified live (the real `control` binary printed "disabled by config" and exited 0 without injecting).

---

# Post-roadmap improvements (2026-07-24)

A second autonomous run, after all five phases were Done, taking on four
maintainer-requested improvements (shorter strip, richer hover label, new sprite
design + never-occluded icons, pets in every tab). Same loop as the phases:
brainstorm (gate waived for the autonomous run) → spec → plan → TDD → one PR
each. Judgment calls recorded below.

## 2026-07-24 — run-wide — Branch these improvements off the Phase-4 stack tip, not `main`
**Question:** CLAUDE.md says "always branch off `main`". But `main` only holds Phases 0–1; Phases 2–4 (the strip, the controller, config — everything these improvements touch) live only on the still-open stacked PR branches, tip `feat/phase-4-config-polish`. Where do the improvement branches start?
**Options considered:** (a) branch off `main` as the convention literally says — but then `render.rs`/`control.rs`/`config.rs` as they exist today aren't present, so the work has nothing to modify; (b) branch each improvement off `feat/phase-4-config-polish` (the stack tip that contains all phases), continuing the existing stack.
**Decision:** Option (b). Each improvement is a branch stacked on the previous improvement's tip (first one off `feat/phase-4-config-polish`), one PR each, base = the branch below. This mirrors the already-documented "Stacked phase branches" decision (the harness still blocks autonomous merges to `main`). The maintainer merges the whole stack — Phases 2→3→4, then improvements 1→2→3→4 — in order; GitHub retargets each PR base to `main` as the one below lands.
**Rationale:** Branching off `main` would make the changes impossible (the code isn't there). The convention's intent — never commit *to* `main`, keep each unit a reviewable PR — is fully honored; only the literal base pointer differs, and self-resolves as the stack merges.

## 2026-07-24 — Improvement 1 — How much is "roughly half", and how to redraw the pets
**Question:** "Cut the strip to roughly half." Half of what, exactly, and what happens to the pets that must still be legible?
**Options considered:** (a) shave a row or two (7→5/6) — too timid for "too tall / don't need that much vertical space"; (b) halve the pet **pixel band** exactly (`PET_PX_H` 12→6) and take the strip 7→4 rows (3 px rows + 1 caption); (c) go smaller still (2 px rows) — illegible.
**Decision:** Option (b). `PET_PX_H` 12→6 (the pet band is *exactly* halved — the cleanest literal reading), strip 7→4 rows (~57%, "roughly half"), keeping the dedicated caption row so Phase 2's "hover never shifts the herd" invariant holds. `config.strip_rows` default 7→4, `place::TARGET_ROWS` 7→4, sprite height guard `h<=12`→`h<=6`.
**Sprite redraw:** the two shipped species (`sheep` 16×12, `goat` 13×12) were redrawn to height 6 (widths kept, so capacity/hit-test math is untouched), preserving each silhouette and per-state frame variation. This is a faithful *vertical compression of the current look*, NOT the new art — the new sprite design + never-occluded icons are Improvement 3, which builds on this budget. Keeping the two changes separate keeps each PR coherent and reviewable.
**No snapshot churn (verified):** the four `render` snapshot tests deliberately use the tiny `test-blob` fixture (4×4), not the embedded sheep/goat, so halving `PET_PX_H` and redrawing the sprites left every snapshot byte-identical (the 4-px blob still fits and draws the same). The real sheep/goat art has no committed snapshot (by the original author's design, to avoid churn on every art tweak); it was eyeballed via a throwaway render (removed) and will get full visual review as a published Artifact in Improvement 3. Gate stays green at 93 tests.

## 2026-07-24 — Improvement 2 — What "workspace/folder + agent name" resolves to
**Question:** Hover shows `Agent::label()` = `name || agent || pane_id`, which is almost always the literal `"claude"` (Felix always runs Claude, rarely sets `name`). The ask: show "the workspace/folder + agent name, i.e. the label shown for that agent in herdr's tab list on the left." What is that label, exactly, and where does it come from?
**Finding (live herdr 0.7.0):** `agent list` carries only ids (`workspace_id`, `tab_id`) — no human labels. The labels live elsewhere: `workspace list` gives each workspace a `label` (e.g. `herdr-pets`, `vbrb-pinb`, `Home folder`) and `tab list` gives each tab a `label` (e.g. `Monorepo UI package`, `XML implementaion`, `Diff`). herdr's left sidebar nests **workspace → tab**, so the row a user reads for an agent is the pair `workspace › tab`.
**Options considered:** (a) show the agent kind/name — that's the useless `"claude"` we're replacing; (b) show only the workspace label — good, but ambiguous when one workspace holds several tabs/worktrees; (c) show the folder basename of `foreground_cwd` — discriminating but not what the sidebar shows; (d) show the full sidebar breadcrumb `workspace › tab`, joined from `workspace list` + `tab list`.
**Decision:** Option (d). Hover = `"<workspace-label> › <tab-label>"`, with a resilient fallback chain (one piece bare if the other's missing → folder basename of `foreground_cwd`/`cwd` → legacy `label()`). This is literally "the label shown for that agent in herdr's tab list on the left," and maximally discriminating (project + task). The generic `"claude"` is gone; a user-set `name`, when present, is already what herdr uses as that tab's label, so the tab label subsumes the "agent name" ask. The join is a small **pure** `Agent::sidebar_label` (unit-tested for every fallback row), so the exact format is trivial to retune.
**Where it runs:** label resolution lives in the **watcher** (`refetch`), which owns the `herdr` CLI seam and is debounced (~250 ms / 2.5 s) — not the 12 fps render loop. `refetch` fetches `agent list` + (best-effort) `workspace list` + `tab list`, builds `id → label` maps (new tolerant `sidebar.rs` parsers), and stamps each agent's new `hover_label` field; `Herd::reconcile` sets `Pet.label = agent.display_label()` (breadcrumb if resolved, else legacy). A failed label fetch degrades to an empty map ⇒ fallbacks ⇒ never worse than before, never a crash. Gate green at 102 tests.

## 2026-07-24 — Improvement 3 — Resolving "shorter strip" vs "never occlude the pet"
**Question:** Improvement 1 halved the strip (pet band 12→6 px, strip 7→4 rows) with overlays still drawn *on* the pet's top row (as the original did). Improvement 3 requires the pet be **never occluded** by its status icon or the `+N` marker, in every state and when crowded. Half-block rows are atomic (2 px) and at crowded density there is no horizontal gap between pets — so where does the icon go?
**Options considered:** (a) offset the icon horizontally into a gap beside the pet — fails when the strip is packed (no gap); (b) shrink the pet band to 2 rows (4 px) and reserve a lane inside the 4-row strip — a 4-px animal isn't legible (violates "keep pets fully legible"); (c) keep the pixel band halved (6 px = 3 rows, the legibility floor) and add **one thin reserved lane** on top for icons + `+N`.
**Decision:** Option (c). Strip layout is now `row0 = badge/bubble/+N lane (no pet pixels) · rows 1–3 = pet band (6 px) · bottom row = caption`. `draw_herd` blits the band into a sub-rect (`y+1`, `height-2`); overlays and `+N` render on row 0. `config.strip_rows` 4→5, `place::TARGET_ROWS` 4→5. Net vs the original: 7→5 rows (still shorter), band 12→6 px (still halved — Improvement 1's core is intact), and now **nothing ever covers a pet**. A reserved lane is the only approach that holds regardless of crowding, and matches the request's own words ("reserve space / reposition icons"). Hover hit-testing is column-based, so the vertical shift doesn't touch the mouse path. Proven by two new render tests (icon on row 0 with no pet pixels there; pet in the band below) + regenerated snapshots.
**Sprite art:** redrew `sheep` (16×6) and `goat` (13×6) to the maintainer's updated side-view design (Artifact `85ac4f4a`): top-lit `L→M→S` woolly body, `#`-framed head with a dark eye `e` and a peach snout `ppp`, four legs, goat horns `hh` + beard. Headers (`frame_ms`/`motion`/`overlay`) are unchanged, so all behaviour (breathe/hop/shake/sway, `Zz`/`!`/`?`, ghost) is preserved — this is art + layout only. The artifact's native frames are ~10–13 px tall; they were **traced down** to the halved 6-px band (the artifact is a look/palette reference, not a size spec) rather than walking back Improvement 1.
**One real rendering gotcha:** legs must use `p` (skin), NOT `#` (outline). The app ships `Theme::Dark`, where the outline colour is `rgb(18,18,18)` — near-invisible on a dark terminal, so `#` legs would make the sheep look legless. The body silhouette reads via its coloured coat, not an outline; `#` is kept only for internal separators (neck/nose) where invisibility is harmless.
**Review:** published a private Artifact (contact sheet: every state × species, a crowded strip with icons + `+7`, a hue-identity row) rendered through a faithful JS mirror of `palette.rs` + the lane layout, for phone review; iterated in place (same URL). Gate green at 104 tests.

## 2026-07-24 — Improvement 3 (revision) — Use the artifact's actual animated poses; give the pet room
**Feedback:** the maintainer pointed out that the artifact (`85ac4f4a`) already contained a **per-state animated** sheep (distinct poses: a lying-down "dozing" pose, a walk cycle, run frames) traced from `sheep_assets_x4`, and my first cut hand-drew a *single* standing pose differentiated only by the overlay glyph — it didn't use those poses.
**Question:** how to use the artifact's real frames, which are ~13–14 px tall, given Improvement 1 had cut the pet band to 6 px?
**Decision (maintainer chose):** give the pet the room its poses need. Copy the artifact's actual frame grids verbatim into the sprites (normalised to 16×14, shorter poses padded on top so feet stay grounded) and map them to herdr states: **idle → the lying-down pose (`row4_f0`) + `Zz`**, **working → the two-frame walk cycle (`row1_f0`/`f1`)**, **done/blocked/unknown → the standing pose** (done hops with a gold `!`, blocked shakes with a red `!`, unknown ghosts + `?`). This bumps `PET_PX_H` 6→14 (7 rows), `strip_rows`/`TARGET_ROWS` back to 9 (1 icon lane + 7 pet rows + 1 caption), and the sprite guard to `h<=14`. It **walks back Improvement 1's halving** for the pet band — an explicit, informed maintainer choice (fidelity to the animated artifact sheep over a shorter strip); the icon lane from the first cut stays, so icons still never cover the pet. Net strip height (9 rows) is ~the original 7 plus the icon lane.
**Bonus:** copying the artifact frames also fixed the earlier legs-visibility worry — the artifact fills the body/legs with coat colour (`M`/`S`, hue-tinted, visible on dark) and uses `#` only as a thin outline, so nothing vanishes on the dark theme. Poses verified via a throwaway engine render (removed): idle sits low/lying, the others stand with four legs + head, goat shows its horns, icons stay in the lane. Gate green at 104 tests. The review Artifact was updated in place with the real frames + a 2-frame walk animation.

## 2026-07-24 — Improvement 4 — Pets in every tab without ever killing work
**Question:** "Make sure the pets pane is in every tab and every workspace — new and existing." The controller only auto-injected into **single-pane** tabs (`needs_strip` required `pane_count == 1`); every pre-existing multi-pane tab was skipped, because the only full-width primitive (`layout.apply`) **kills every pane's process** (the locked Phase-3 constraint). How to cover multi-pane tabs without that?
**Finding (live herdr 0.7.0):** `herdr pane split` is non-destructive but only spans the width of the pane it splits. `herdr pane layout --pane <p>` returns the whole tab's geometry — the tab `area` and every pane's `rect`. That's enough to find a **full-width bottom pane** (spans the tab width, touches the bottom edge) and split *it* `down` non-destructively for a **full-width** strip. Single-pane tabs are the trivial case; the very common "content on top + full-width terminal/agent across the bottom" multi-pane layout also has one. Only tabs whose bottom edge is split into side-by-side **columns** have none.
**Options considered:** (a) keep skipping multi-pane tabs (status quo — fails the ask); (b) split an arbitrary/column pane on multi-pane tabs (a non-full-width strip stuck under one column — ugly, fights "glanceable/unobtrusive"); (c) inject into any tab with a full-width bottom pane by splitting that pane, and leave columned-bottom tabs to on-demand `place`.
**Decision:** Option (c). `needs_strip` now means "no strip yet" (any pane count); a new pure `find_bottom_strip_target` picks the full-width bottom pane from the layout, and `sweep_once` injects there (or skips when there's none). `inject_strip` takes the target pane + its own row count so `slim_ratio` sizes the strip relative to the pane it splits, not the whole tab. This is a large coverage jump — **every single-pane tab plus every top+full-width-bottom multi-pane tab, across all workspaces** (`tab list` is session-wide) — while never using `layout.apply` and never killing a process (`pane split` preserves it; the split pane just gets shorter). Columned-bottom tabs remain on the on-demand `place` — the honest, bounded limit of "never disturb running work," consistent with GOAL.md's existing compromise.
**Validated on real data:** a mirror of `find_bottom_strip_target` run over 16 live tabs found a full-width bottom pane in 15 and correctly classified the one genuinely columned tab (`w1:t1D` "XML implementaion") as skip; label de-dup (`tabs_with_strip`) still skips already-stripped tabs first. Per the Phase-3 restraint I did NOT unleash the unbounded controller on the maintainer's ~40-tab session; coverage is proven by unit tests over the CLI seam + this real-layout classification. Gate green at 106 tests.
**Documented limit (unchanged, deferred):** the controller still has no plugin-start hook (herdr doesn't fire one — Phase 0 Spike B), so "always there" requires starting it once per session via the `start-pets-controller` action / `herdr-pets control`. Auto-start remains deferred.

## 2026-07-24 — Improvement 3 (revision 2) — Slim it down for real; bottom-anchor so nothing clips
**Feedback (from the live demo):** at 14 px / 9 rows the sheep were **clipping at the strip's bottom edge**, and the strip was still "way too big" — the maintainer wants "at least half" the height, and the pets a bit *smaller than the strip* so a jump can't exceed the bounds ("it's just to have a status underneath everything of all the running agents").
**Two root problems:** (1) the sprite was the same height as the band and **top-anchored**, so `motion_offset`'s hop (up to 2 px) pushed it past the edges and the lying pose's belly sat flush against the caption row; (2) 9 rows is too tall for a glanceable status line.
**Decision:** make it a genuinely slim status strip and guarantee no clipping.
- **Height:** `PET_PX_H` 14→6 (3-row band); `strip_rows`/`TARGET_ROWS` 9→5 (1 icon lane + 3 pet rows + 1 caption). Roughly half the previous strip. This reverses the "give it room" revision above — the maintainer, seeing it live, chose slim over pose fidelity ("they really don't have to be that big").
- **Sprites:** redrawn small at **12×5** (was 16×14) — a compact side-view sheep, a low lying-down idle, a two-frame walk; goat = same + a horn. Narrower too, so they don't read as stretched at this height.
- **No-clip invariant:** sprites are **≤5 px** (guard tightened to `h<=5`, one px shorter than the 6 px band) and the renderer now **bottom-anchors** them (`floor = PET_PX_H - frame_h`); motion (`dy<=0`) lifts the pet *up into* that 1 px of headroom instead of off the top/bottom. The hop/shake vertical amplitude is also capped at 1 px (`anim.rs`) so it exactly fits the headroom. Feet always rest on the band floor; a hop never crosses into the icon lane or the caption row. Verified via a throwaway render (removed): idle lies low, standing pets show legs, mid-hop frames stay inside the band.
Gate green at 104 tests; snapshots regenerated (pets now bottom-anchored). README + review Artifact updated.

## 2026-07-24 — Improvement 3 (revision 3) — herdr's minimum pane height; bottom-align the strip
**Finding (live, herdr 0.7.0):** when reloading the demo I could not get the strip pane below ~9 rows on the maintainer's 86-row display, no matter the split ratio. Probing `pane split` at ratios 0.90–0.99 all clamped the new pane to ~8–9 rows: **herdr enforces a minimum pane height of ~10% of the tab** (≈9 rows on an 86-row tab; ≈7 on a 64-row tab — which is why the old strips were 7). There is no herdr config to lower it (`~/.config/herdr/config.toml` has only `sidebar_min_width`). So the plugin **cannot make the strip pane shorter than herdr's minimum**; a `strip_rows` / `TARGET_ROWS` below that is silently clamped up by herdr. The absolute row count therefore scales with the display — slim on a normal terminal, taller on a very tall one.
**Consequence + fix:** since the pane can be forced taller than the content needs, `draw_herd` now **bottom-aligns** the whole strip — caption on the bottom row, pet band just above it, icon lane just above the band — so the content reads as a slim status line and any extra rows fall at the top (blending with the pane above) instead of leaving a dead gap in the middle. This is height-independent: identical to before on a tight 5-row pane, tidy on a herdr-inflated one. Snapshots regenerated. The remaining "strip is tall on my display" is a herdr pane-minimum limit, surfaced to the maintainer rather than worked around.

## 2026-07-24 — Kitty-graphics rendering backend — opt-in upgrade added; supersedes the v3 sprite slimming

**Question:** The maintainer wanted pets small on screen but with the full detail of the traced artifact sprites (16×14 v2 art). Half-block rendering can't do both — each sprite pixel is locked to 1 char cell wide × ½ cell tall, so "smaller" in half-blocks can only mean "fewer pixels," i.e. less detail. That tradeoff is exactly what drove Improvement 3's v3 12×5 hand-drawn slimming, which the maintainer disliked (it discarded the artifact art). Is there a way to get small *and* detailed?

**Research + live spikes (2026-07-24):** the option space was investigated (custom fonts, image protocols, environment ground truth), with three findings that changed the picture:
- The maintainer's terminal is **Ghostty**, which natively supports the **kitty graphics protocol** (true pixel images: any size, full color, crisp, animatable).
- herdr sits between the plugin and Ghostty and vendors `libghostty-vt`, with first-class kitty-graphics support (including the unicode-placeholder / virtual-placement feature that survives a multiplexer) — gated behind an **experimental, off-by-default** flag (`[experimental] kitty_graphics = true` in `~/.config/herdr/config.toml`), which additionally requires `herdr server reload-config` **and a client detach + reattach** (rendering is negotiated client-side).
- Custom fonts render smooth-not-crisp and are fiddly/unproven on Ghostty; Sixel is a dead end (Ghostty declined it); half-blocks stay crisp but can't be made small. **Kitty graphics is the only path to small + crisp + detailed.**

Proven live via throwaway spikes in a herdr pane: (1) a 4-color image renders through herdr → Ghostty once the flag is on and the client is reattached (before that, herdr silently drops the escape); (2) the actual v2 artifact sheep/goat, rasterized with the plugin's palette, render crisply at small sizes — the maintainer chose scale ≈ 7 (image px per sprite px); (3) animation is smooth through herdr's passthrough — a walking sheep that roams and flips to face its direction of travel had no flicker or trails at ~12 fps, drawing each frame and deleting the previous image id. See memory `herdr-pets-kitty-graphics-works` for the durable summary.

**Options considered:** (a) keep the v3 12×5 slimmed sprites as the only look, accepting the detail loss; (b) chase custom fonts or Sixel — ruled out by the spikes above; (c) add a kitty-graphics backend as a second, opt-in `PetRenderer` behind runtime detection, restore the sprites to the full-detail 16×14 art, and keep half-block as the universal, always-available fallback.

**Decision:** Option (c). This directly extends `GOAL.md`'s locked decision — *"Universal first … Fancier rendering (e.g. Kitty graphics) is only ever an opt-in upgrade on top, never a requirement"* — bringing that deferred stretch goal forward. Concretely:
- `KittyRenderer` (`src/kitty_render.rs`) implements `PetRenderer` by transmitting each distinct sprite frame once (cached by species/status/frame/flip/hue) and re-placing it every frame via kitty graphics escapes, deleting placements for departed/overflowed pets.
- `TerminalCaps` (`src/caps.rs`) probes for kitty support at startup (writes a kitty query, polls stdin briefly for the reply) behind a `RealCaps`/`FakeCaps` seam so tests never touch a real terminal.
- `select_renderer` (`src/render.rs`) resolves `Config.renderer` (`auto` | `kitty` | `half-block`) against the probe: forced kinds win outright; `Auto` probes and falls back to half-block on no reply (flag off, non-kitty terminal, timeout).
- **This supersedes the Improvement-3 v3 12×5 sprite slimming** (`docs/decisions.md`, "Improvement 3 (revision 2)"): the shipped sprites are **restored to the full-detail 16×14 v2 art** (`PET_PX_H` back to 15, matching "Improvement 3 (revision)"). The **half-block fallback is accepted as taller** than the v3 slim strip — it's the honest cost of keeping one shared sprite asset across both backends rather than maintaining two divergent art sets.
- The upgrade is **fully dependent on herdr's experimental flag and fallback behavior**: with the flag off (the default), `auto` transparently uses half-block; there is no user-visible breakage, only a missed crispness upgrade until the maintainer opts in.

**Scope note:** overlays (bubbles/badges) and the `+N` overflow marker are not yet drawn for the kitty path (a known, tracked gap — the hover caption still works for both backends since it's drawn separately by the render loop). Real terminal-cell-size detection (`CSI 14 t`) is deferred; `KittyRenderer::new_stdout` uses a conservative `(8, 16)` px fallback. Live verification in a real kitty-capable terminal (crispness, animation, hover/click, and the half-block fallback) is a separate, maintainer-assisted step — not claimed as verified by this entry.

## 2026-07-24 — Kitty motion + pixel-icon overlays — closes part of the Scope note above

**Finding (live comparison, maintainer's Ghostty pane):** the artifact mockup used to explain agent states (idle breathe, working hop+wander, done hop+badge, blocked shake+badge, unknown sway+bubble) didn't match what the maintainer actually saw in their kitty-rendered strip: no bob/hop/shake/sway at all, and no overlay glyph either. Tracing it down: `src/kitty_render.rs`'s `render_pets` never called `motion_offset` (only the half-block path in `src/render.rs` did) and never drew an overlay — the "known, tracked gap" from the Scope note above covered the overlay half of this, but the missing body motion wasn't previously documented.

**Design ask:** replace the text-glyph overlay concept with a small pixel-art icon (Zz / `!` / `?`, built from filled pixels, no bubble/badge background) that floats independently of the body, and swap `blocked`'s `shake` for a snappier dock-icon-style `bounce` jump.

**Decision:** implement both for the kitty backend specifically (not half-block): its reserved overlay lane is locked to 1 terminal row (`place.rs::TARGET_ROWS = 5`, ~2 half-block pixels) — not enough to draw a legible bespoke glyph — while kitty places a real raster image at full pixel resolution, and it's what the maintainer's Ghostty pane actually uses.

- `src/anim.rs`: `Motion::Shake` → `Motion::Bounce` (steeper takeoff via `sin.max(0).powf(0.6)`, same 1px cap as `Hop`); new `icon_wave_offset` — a full sine (rises, then genuinely returns) plus a slow lateral drift, driving the overlay icon independent of the body's own motion/phase.
- `src/pet.rs`: new `icon_phase` field + `advance_icon`, unconditional (unlike `phase`, never pinned to 0 by a static `frame_ms=0` state), so an icon keeps floating under `done`/`unknown` too.
- `src/icon.rs` (new): `IconKind` (Sleep/Alert/Question) with fixed 5–7px bitmaps, `rasterize_icon`. Alert is always red regardless of theme (a status signal, not a themed decoration); Sleep/Question are theme-aware neutral ink.
- `src/kitty.rs`: new `place_cropped`/`Crop`, using kitty's `x=`/`y=`/`w=`/`h=` source-crop keys.
- `src/kitty_render.rs`: pet and icon images are now transmitted once onto a **padded** canvas (`pad_frame`, `MOTION_PAD`/`ICON_PAD` = 2 sprite/icon px), and each frame pans a same-size **crop window** over that static image to animate — no retransmit cost per frame. Icon placements are tracked separately from pet placements (`icon_placements`) since a pet can lose its overlay on a status change while staying visible (idle → working), which must tear down the icon without touching the pet.

**Still open:** the `+N` overflow marker remains undrawn for kitty (unaffected by this change). Live verification in a real kitty-capable terminal is, as before, a separate maintainer-assisted step — this entry only claims the escape-byte-level unit tests pass.

## 2026-07-24 — Deterministic, wall-clock-driven animation — replaces the per-process RNG simulation

**Finding:** the maintainer noticed pets in the same agent didn't line up across different tabs/panes — sleeping (idle) pets were usually close, but moving (working) pets were often in completely different spots. Root cause, traced in `herd.rs`: each `herdr-pets render` process is a fully independent OS process with its own `Herd` simulation. All of them seeded their RNG identically (`Lcg::new(0xC0FFEE)`), but a pet's spawn position *and* its wander target both drew from one shared, order-sensitive RNG stream, consumed by both `reconcile` and `step` — real per-process tick timing (not the shared seed) decided the actual sequence, so independent processes diverged from the first tick. Non-working pets never touched the RNG in `step()` at all (`roam > 0.0` short-circuits), so their spawn-time draw stuck — explaining why idle pets looked "usually" aligned and working pets didn't.

**Options considered:** (a) a shared coordinator process (extend the `control` watchdog to compute one canonical herd and broadcast it over local IPC to every pane) — real IPC/protocol work, and would make every pane's rendering *correctness* hard-depend on `control` running, which today is optional (only auto-placement depends on it); (b) make position and animation phase a **pure function of `(terminal_id, status, wall-clock time)`** — no shared state, no coordination, computed independently and identically by every pane.

**Decision:** option (b). herdr panes all run as processes on the *server* side (thin clients just attach to view them), so even under `herdr --remote` every process shares the exact same machine clock — no cross-machine skew to worry about.

- New `src/motion.rs`: `animate(terminal_id, status, state, now_ms) -> Animated` — a pure function. A pet's "personality" (wander phase/period, rest position/facing, animation phase offset, icon phase offset) is derived once per call from `identity::unit_hash(salt, terminal_id)`, not accumulated. Position is returned as `x_fraction: 0.0..1.0` (a fraction of the walkable width), not a pixel — different tabs have different widths, so each renderer multiplies by its own local `max_x`.
- `Herd`/`Pet` simplified to match: `Herd::step`, `Lcg`/`Rng`, and `Pet.x`/`.target_x`/`.phase`/`.icon_phase`/`.facing_left` are gone. `Herd::reconcile` is now just add/update/remove by `terminal_id` — no RNG, no `strip_w`/`pet_w` params.
- `render.rs`/`kitty_render.rs`: `draw_herd`/`render_pets`/`pet_at_column` all take an explicit `now_ms: u64` and call `motion::animate` fresh every frame instead of reading accumulated pet state. `run_loop` drops `simulate_tick` entirely and reads the wall clock once per tick (`SystemTime::now()`), or freezes at a fixed instant (`0`) under `reduced_motion` — which falls out of the pure-function design for free, no separate code path.

**Trade-off, deliberately accepted:** the old pairwise "nudge working pets apart so they don't overlap" behavior is gone. It depended on the *current set of visible pets*, which differs per pane (different tab widths → different overflow capacity) — keeping it would have reintroduced exactly the per-pane divergence this change exists to remove. Occasional brief visual overlap between two working (or several idle, if their identity hashes land close together) pets is the accepted cost of exact cross-pane sync.

**Verification:** unit tests assert the core promise directly (`motion::tests::same_inputs_yield_the_identical_result_every_time`) plus the derived properties (working pets move over time, non-working pets don't, distinct agents don't move in lockstep, a static state's body motion pins but its icon still floats). Snapshot tests were regenerated (positions are no longer artificially spaced by spawn order, so the pixel layout looks different, not wrong). Live-verified across the maintainer's actual multi-tab, multi-workspace herdr session by restarting every `herdr-pets` strip pane and confirming the controller's non-destructive re-injection still works end to end.

## 2026-07-26 — Packaging: fetch-or-build install (ships the deferred Phase 4 item)

**Decision:** Ship the packaging/release that Phase 4 deferred (see the
2026-07-23 entry). `scripts/build.sh` now downloads a prebuilt binary for the
platform and only runs `cargo build --release` as a fallback, so installing
needs **no Rust toolchain** on the four published targets
(`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu`). A tag-triggered `release.yml` cross-compiles and
publishes those binaries to a GitHub Release; `ci.yml` (fmt/clippy/test) stays
the PR gate.

**Chosen over:** source-build-only (fails the "easy for others" goal — needs
Rust) and prebuilt-only (drops the free robustness of the fallback). Fetch-or-
build gives "no toolchain for the common case" and "never hard-fails" from one
design. Spec: `docs/superpowers/specs/2026-07-26-packaging-and-install-design.md`.

**Risk accepted:** `aarch64-unknown-linux-gnu` is cross-compiled with the
`gcc-aarch64-linux-gnu` linker (no C deps in the tree, so this is safe). If that
matrix leg ever proves fragile, drop it — arm64-Linux users then hit the
source-build fallback, still a working install where Rust is present.

**Manual maintainer step:** cutting the tag/release is done by the human (repo
convention: no autonomous commits/pushes) — see the release checklist in the
plan.

## 2026-07-27 — Rename: herdr-pets → herdr-herd, "pet" → herd/sheep

**Decision:** Rename the project from `herdr-pets` to `herdr-herd`. The product
is about *herding your herd of agents* and making it visual (a flock of sheep),
not about "pets." Two layers changed together:

- **Package/repo:** crate/binary `herdr-pets` → `herdr-herd`, lib ident
  `herdr_pets` → `herdr_herd`, env vars `HERDR_PETS_*` → `HERDR_HERD_*`, release
  asset/download slug, manifest `id`/`name`. `release.yml` publishes under the
  dynamic `$GITHUB_REPOSITORY`, so it tracks the repo rename automatically;
  `build.sh`'s download URL hardcodes `vandemaelefelix/herdr-herd`.
- **Domain/concept:** the individual-unit type `Pet` → `Member`
  (`src/pet.rs` → `src/member.rs`, `pets: Vec<Pet>` → `members: Vec<Member>`),
  config key `pet_scale` → `member_scale`. `Herd` stays the collection type. UI
  framing moves to "herd": pane id/title `pets`/`"Pets"` → `herd`/`"Herd"`,
  actions `place-pets`/`start-pets-controller` → `place-herd`/`start-herd-controller`,
  the strip is a "herd strip," and the pane-label recognizer tracks `"Herd"`.
  Living docs (README, GOAL, CLAUDE) reframe the flavor to sheep — and the README
  now truthfully names the two shipped species (sheep + goat) instead of the old
  aspirational list.

**Chosen noun (`Member` over `Sheep`):** `Herd` was already the collection type,
so the unit needed a new word. `Member` is generic and survives future non-sheep
sprites; "sheep" stays as flavor in prose. Insta snapshot files were renamed to
the new crate prefix (and one test name `..._focused_pet_...` → `..._focused_member_...`).

**Scope:** code, config, and living docs were renamed. Dated design artifacts
(`docs/superpowers/**`, handoffs, `docs/PLAN.md`, earlier entries in this file)
keep `herdr-pets` as an honest historical record of the name at the time. The
`.claude/skills/**` example paths were left as-is.

**Manual maintainer steps (outward-facing, not done by the agent):** rename the
GitHub repo `vandemaelefelix/herdr-pets` → `vandemaelefelix/herdr-herd` (GitHub
redirects the old URL, but `build.sh`'s hardcoded slug and the README install
one-liner assume the new name), and rename the local working directory. Done
pre-release (v0.1.0 unshipped), so the config-key/action-id/env-var changes break
no installed users.

## 2026-07-27 — Resume-Working ease: stop the position teleport when a sheep starts working again

**Finding:** the maintainer noticed a sheep's position jumps suddenly on a status
change. The existing `Anchor` (see the 2026-07-27 deterministic-animation entry
and the anchor already in `motion.rs`) only handled *leaving* `Working` — it
freezes a settling sheep in place instead of teleporting it to the identity rest
spot. But *re-entering* `Working` **cleared** the anchor, and
`motion::animate` then placed the sheep at `wander_segment(u)`, where `u` is a
pure function of absolute wall-clock time. That free-running cycle bears no
relation to where the sheep was resting, so the instant it resumed work it
snapped to wherever the global amble clock happened to be — the teleport the
maintainer saw. (Working→Idle itself was already jump-free; the visible jump is
on the round-trip, when work resumes.)

**Options considered:** (a) *walk out from the rest spot* — anchor the wander
cycle's origin to the resume position/instant (a full mirror of the leave-anchor).
Smoothest, but reintroduces per-pane state into the cycle itself and breaks the
strict cross-pane agreement the deterministic-animation redesign exists to
protect. (b) *ease from rest into the cycle* — keep the stateless global cycle,
but blend `frozen_x → wander_segment(now)` over ~1s using the (re-stamped)
anchor, then hand off to the plain cycle. (c) leave it — rejected, it's a real
visible glitch.

**Decision (maintainer chose): option (b).** On re-entering `Working`,
`Herd::reconcile` now **keeps** `frozen_x` (the rest spot) and **re-stamps**
`settled_at_ms` to the resume instant instead of clearing the anchor. A new
shared `motion::working_position(terminal_id, now_ms, anchor)` computes the
Working position for both `animate` (draw) and `reconcile` (leave-capture): it's
the plain wander cycle, `smoothstep`-blended out from `frozen_x` for the first
`RESUME_EASE_MS` (~1s) after resuming. It's C0-continuous at both ends — exactly
`frozen_x` at the resume instant (matches the last idle frame, so no jump) and
exactly the free cycle once the window elapses. During the ease the sheep faces
its travel direction and cycles its legs, so it reads as walking out, not sliding.

**Trade-offs, deliberately accepted:**
- **Cross-pane divergence during the ~1s ease.** A pane that observed the resume
  shows the walk-out; a fresh/late-attached pane (no anchor) shows the plain
  cycle from the first frame with no ease-in. This is the *same* class of
  per-pane cosmetic tradeoff the leave-anchor already makes, and it is bounded:
  because the ease hands back to the stateless cycle after `RESUME_EASE_MS`, all
  panes re-converge exactly once the window elapses. The core "every pane agrees"
  invariant holds in steady state; only the ~1s transient can differ.
- **Anchor is now dual-purpose.** The same `Anchor` means "frozen here" while
  non-Working and "easing out from here" while Working; `animate` disambiguates
  by status. Documented on the `Anchor` type and `reconcile`.
- **Facing may flip once at resume** (ease uses the travel direction, which can
  differ from the idle rest facing). Accepted: a single facing flip is far less
  jarring than the position teleport it replaces, and no worse than before.

**Verification:** TDD, red-then-green. New unit tests: `motion` — resume starts
at the rest spot, and converges back to the plain cycle after the window; `herd`
— re-stamp-not-clear on resume, unanchored-stays-unanchored, and (the reason for
the shared helper) leaving Working *mid-ease* freezes at the on-screen eased
position, not the raw cycle. The pre-existing `reconcile_clears_the_anchor…`
test was updated to the new re-stamp behavior. Gate green: 232 tests, clippy
clean, fmt clean.

## 2026-08-06 — Dev test harness: dedicated session + feature-gated build marker

**Context:** Testing herdr-herd meant running it in the one session the user
works in, so a dev controller injected strips over live agents. And because the
installed plugin is pinned to a release commit while a local build may be several
fixes ahead, "is my fix in this pane?" was unanswerable by looking.

**Decisions:**

- **No new isolation mechanism.** Session scoping was already load-bearing:
  `socket::socket_path` reads `$HERDR_SOCKET_PATH`, `LiveHerdr` shells out to a
  CLI that inherits it, and `controller_lock_path` hashes the socket path into
  the lock filename. The harness is just "second session, controller pointed at
  its socket". The controller runs as an outside socket client; it never needs
  to live in a pane.
- **Cargo feature `dev-marker`, off by default,** rather than an env var or
  `debug_assertions`. It is the only option where the marker code is absent from
  a shipped binary: an env var ships the code and can be flipped on by accident,
  and `debug_assertions` would force dev builds into the debug profile, which we
  do not want for animation smoothness.
- **The build stamp comes from `build.rs` with no `rerun-if-changed`
  directives.** With none present Cargo reruns the script whenever any package
  file changes, so every rebuild restamps and two dev builds of one commit are
  still distinguishable. Dirty trees are flagged `*`, not `+`, which the
  overflow counter already owns in the same lane.
- **The marker draws from `MemberRenderer::draw`, not `draw_herd`.** This keeps
  the herd itself feature-independent, so the layout snapshots keep asserting
  the shipped strip whichever way the crate is built. Mirrors the kitty path.
- **`HERDR_HERD_CONFIG_DIR` instead of a second plugin id.** An earlier sketch
  linked the dev checkout as `herdr-herd-dev` for a separate config dir.
  Rejected: the id lives in `herdr-plugin.toml`, so a second id means a
  duplicate manifest, and the dev build need not be a registered plugin at all.
  One env var buys the same isolation.

**Trade-offs, deliberately accepted:**
- **`jq` is a dev-only dependency** of `scripts/herd-test.sh`, used to read
  `herdr session list --json`. The script fails with a clear message without it.
- **The marker costs overlay-lane columns in dev builds,** so a hover caption
  truncates earlier there than in a shipped build. `marker::reserved_cols()` is
  `0` when the feature is off, so shipped layout is byte-identical.
- **Config sharing is opt-out, not opt-in.** With `HERDR_HERD_CONFIG_DIR` unset,
  dev and installed builds share one config, which is usually what you want when
  testing against your real settings.

**Verification:** TDD, red-then-green throughout. Gate green: 242 tests default,
247 with `--features dev-marker`, clippy clean, fmt clean.

## 2026-08-06 — Hot reload on binary change, and one strip per tab

**Context:** With the dedicated test session in place, the remaining manual step
was restarting strips after a rebuild: strip panes and the controller are
long-running processes still executing the binary image they started from, so a
rebuild changed nothing visible.

**Decisions:**

- **The controller watches its own binary and re-execs.** Each sweep it compares
  `binary_stamp(self_exe)` against the stamp taken at startup. On a change it
  closes its strips and `exec`s itself, so a change to `control.rs` or
  `place.rs` is hot too, not just renderer changes. One mechanism covers both.
- **On by default, in shipped builds too.** Chosen over gating it behind
  `dev-marker`: today a user who updates the plugin keeps running the old strips
  until they restart herdr, which is a real (if quiet) bug. The cost is a brief
  strip blink at plugin-update time.
- **The lock is not dropped before `exec`.** Rust opens files `O_CLOEXEC`, so a
  successful `exec` releases the controller lock at exactly the right moment —
  past the point of no return — and the successor image can take it. Dropping it
  early would open a window where a second controller could start while this one
  is still alive.
- **A failed re-exec adopts the new stamp and keeps sweeping.** Otherwise a
  binary that cannot be exec'd would close every strip on every sweep forever.
  Stale-but-stable beats flapping, and the sweep re-injects what it just closed.
- **Reload is scoped to strips the controller owns** (label `herdr-herd`), not
  everything [`is_strip_label`] matches. The sweep can only re-create what it
  injected; closing a manifest-opened `Herd` pane in a columned-bottom tab would
  lose that strip for good.
- **The sweep reaps duplicates.** `plan_reap` closes every strip after the first
  in each tab, so "one strip per tab" is enforced rather than merely intended.
  Injection alone could not guarantee it — it only ever adds.
- **An unlabellable injection is rolled back.** If `pane rename` fails after the
  split, the new pane is closed and the injection reported failed. That orphan
  was the main way a tab ended up with two strips: unlabelled, it was invisible
  to every later sweep, which then injected another.

**Trade-offs, deliberately accepted:**
- **Reload is a blink, not a seamless swap.** Strips close and come back a sweep
  later. Making it seamless would mean injecting the new strip before closing the
  old, which transiently violates the one-strip-per-tab invariant we just made
  explicit.
- **`plan_reap` keeps the first strip in `pane list` order,** not the
  best-placed one. Deterministic and testable; there is no signal available that
  would make a smarter choice.
- **mtime, not content hash,** for change detection. Cheap, and cargo/`build.sh`
  both install by atomic rename, so a torn read is not a concern.

**Verification:** TDD, red-then-green. New unit tests cover reaping (none, one,
several, per-tab), the sweep's close calls, rollback of an unlabellable
injection, `binary_changed`'s unreadable-stamp handling, and reload's ownership
scope. The control loop itself stays a thin untested shell over tested parts.
Gate green: 252 tests, clippy clean, fmt clean.

## 2026-08-06 — A dead strip is a corpse, not a strip

**Context:** Reported as "the red hat is not always on the active agent's sheep —
there is always a sheep with a hat, but not the focused one, and it does not
match the agents sidebar."

**Root cause (investigated, not guessed):** the hat logic is correct at every
layer. `herdr agent list` reports exactly one `focused` agent and it agrees with
`pane list`; `Herd::reconcile` assigns `focused` on both the update and insert
paths; `visible_and_hidden` protects the focused member from overflow; the kitty
`ImgKey` includes `focused` so a hatted image is never reused for another sheep.

The strips were simply **not running**. Every strip pane in the reporting
session had `herdr-herd render` exited and its shell back in the foreground
(`process-info` → `name: zsh`, against `name: herdr-herd` for a live one). Under
the kitty backend that is invisible: placements are only deleted by `teardown` on
a *clean* exit, so the last frame drawn stays frozen on screen — including the
hat, pinned to whichever agent was focused at the moment the renderer died.

It never self-healed because the pane **keeps its label** when the renderer
exits, and `plan_injections` treats any strip-labelled pane as proof the tab is
covered. Liveness was "is there a labelled pane", not "is there a running
renderer".

**Decisions:**

- **Inject with `exec`.** `pane run <id> "exec '<self_exe>' render"` makes the
  renderer replace the pane's shell, so when it exits the pane exits with it.
  The corpse cannot form in the first place. This is the primary fix.
- **Also reap dead strips.** `exec` prevents new corpses but cannot heal the ones
  already out there, so each sweep checks `pane process-info` per controller
  strip and closes any whose renderer is gone. Belt and braces, and it is what
  repairs an existing session.
- **Close now, inject next sweep.** Re-injecting in the same pass would race the
  layout that sweep already read.
- **Unreadable process-info counts as live.** A transient failure must never
  close a healthy strip, so every parse failure, missing field, and empty process
  list resolves to "live".

**Trade-offs, deliberately accepted:**
- **One `process-info` call per strip per sweep.** Comparable to the per-tab
  `pane layout` the sweep already makes, at a 3s cadence.
- **A brief race on a just-injected strip.** A strip is labelled moments after
  `pane run`, so a sweep landing in that window could see a shell and close it.
  Self-correcting (the next sweep re-injects) and made rare by `exec`.
- **`teardown` on abnormal exit is still not solved.** A SIGKILLed renderer
  leaves its images on screen until the pane closes. `exec` makes the pane close
  with it, which is what actually clears them.

**Verification:** TDD, red-then-green. Unit tests cover live/dead/unreadable
process-info, the sweep closing a dead strip, and the control case that a live
strip is never closed. Verified live in the `herd-test` session: killing a
renderer removed its pane entirely (`w1:pD` gone, not a shell) and the sweep
injected `w1:pG` to replace it. Gate green: 257 tests, clippy clean, fmt clean.

## 2026-08-19 — The kitty renderer does not own the terminal

**Context:** Issues #29, #28, #30 and #46, all from the 0.2.1 code review. They
are one root cause wearing four hats: `KittyRenderer` was written as though it
were the terminal's only client. Every strip pane is its own process, but they
all forward their escapes to ONE outer terminal, so image ids, deletes and
image memory are a single shared, terminal-global namespace.

**Decisions:**

- **Partition the id space by process, do not negotiate it.** kitty's `I=`
  image-number mechanism exists precisely for uncoordinated clients, but it
  requires reading the terminal's reply back — which the strip cannot rely on:
  herdr forwards our escapes, we suppress replies with `q=2`, and the render
  loop has no reader for them. So each pane claims one of 65535 blocks of
  65536 ids, mixed from its pid and startup instant (`src/kitty_ids.rs`).
  Collision is possible but is a ~1-in-65535 coin flip per pair of live panes,
  against a 100% collision rate before.
- **Placement ids get their own counter.** They were allocated per member per
  frame off the same counter as image ids, so ~60 ids/second. Any block-based
  scheme would have been exhausted in minutes. Placement ids are scoped to
  their image id in the protocol, and image ids are now disjoint per pane, so a
  plain wrapping counter is sufficient.
- **`a=d,d=A` is deleted from the crate, not just avoided.** `kitty::delete_all`
  is gone and replaced by `delete_image(id)` (`a=d,d=I`). Keeping the builder
  around as an unused footgun invited exactly the regression #28 describes.
- **Eviction counts frames, not milliseconds.** The obvious TTL clock is the
  `now_ms` already threaded through `render_members`, but reduced-motion mode
  pins it to 0 for the life of the process, which would silently disable
  eviction for anyone using that setting. The render loop ticks at a fixed
  ~12 fps, so a frame counter is an equivalent clock that cannot be frozen.
- **The resize purge stays.** With `d=A` gone, the original reason for purging
  on resize (another pane having wiped our images) is weaker, and images are
  placed with an explicit `c=`/`r=` cell footprint, so a resize does not
  actually need new pixels. Removing it would save the retransmission burst
  #46 measures, but it is a behaviour change outside these four issues and the
  existing test pins it. Left as a follow-up.
- **`member_scale` defaults to 4.** A member is displayed in ~7x4 cells from a
  16x19 sprite-pixel crop window; scale 4 transmits about one pixel per screen
  pixel. Scale 7 was 2-3x that, for a nearest-neighbour upscale the terminal
  does anyway.

**Trade-offs, deliberately accepted:**
- **Block collision is unlikely, not impossible.** Two panes whose pids and
  start instants happen to mix into the same block still share ids. A
  negotiated scheme would need a reply channel we do not have.
- **An abnormally killed pane leaks its images.** Nothing frees them until the
  terminal is reset. That was already true, and `d=A` was never a safe way to
  clean it up.

**Verification:** Unit tests, each confirmed red against the old behaviour by
re-introducing it (a shared id block; a `d=A` purge; eviction disabled). Two
`KittyRenderer` instances are driven against separate sinks so the cross-pane
properties are actually observable. Gate green: 273 tests, clippy clean, fmt
clean. NOT verified live in a multi-pane session with a real terminal — the
id-disjointness and the `d=I` frees are proven at the escape-sequence level
only.
