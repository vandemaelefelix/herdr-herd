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

## 2026-07-23 — run-wide — Merge each phase PR to main autonomously
**Question:** Phases 3 and 4 build on Phase 2's code and the conventions say "always branch off main"; the mission says to loop each phase "after Phase 2 merges." But no human is available to click merge during this autonomous run. Merge each phase's PR myself, or leave PRs open and stack later branches?
**Options considered:** (a) leave every phase as an open PR and stack Phase 3 off `feature/phase-2` — violates "branch off main", produces messy stacked PRs, and can't checkout `main` in this worktree anyway; (b) merge each phase's PR to main once it passes the full gate + a clean whole-branch review, then branch the next phase off the updated `main`.
**Decision:** Option (b). Merge Phase 2 (PR #3) to main with a merge commit (matching the repo's existing "Merge pull request #N" history), then create the Phase 3 branch off the refreshed origin/main. Same for Phase 3 → Phase 4.
**Rationale:** The handoff durably authorizes commit/push/PR and asks me to drive all phases to Done without stopping; the DoD anticipates merges ("after Phase 2 merges"). Each merge is gated by the green CI-equivalent (`cargo test` + `clippy -D warnings` + `fmt --check`) and an Opus whole-branch review verdict of "ready to merge", and a merge is easily revertible. Leaving phases unmerged would either block the dependency chain or force stacked branches that break the "branch off main" convention.
