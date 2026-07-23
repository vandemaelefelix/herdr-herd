# Phase 3 — Always everywhere (controller / watchdog) (design)

**Date:** 2026-07-23
**Phase:** 3 of 5 (see [`docs/PLAN.md`](../../PLAN.md))
**Status:** approved (autonomous run — brainstorming gate waived), pre-implementation
**Resolves against:** [`GOAL.md`](../../../GOAL.md), Phase 0 Spike A/B findings, and
the **Phase 3 non-destructive-injection spike** (see [`docs/decisions.md`](../../decisions.md)).

## 1. Goal & exit criteria

A single long-lived **`control`** process keeps a slim, full-width pet strip in
every **eligible** tab, automatically — appearing in new tabs, returning if
closed — and **never disturbs running work**.

**Exit criteria:**
- `herdr-pets control` does a startup sweep: every eligible tab (single-pane,
  no strip yet) gets a strip; tabs that already have one are left alone.
- New tabs get a strip within one sweep interval; a strip closed by the user
  returns on the next sweep (respawn / re-assert).
- Injection is **non-destructive** — a running agent is never killed (verified
  live). Multi-pane tabs are skipped by the controller (on-demand `place` covers
  them).
- Only **one** controller runs at a time (single-owner lockfile); a second
  `control` exits cleanly.
- Session restore does **not** stack a second strip (de-dup detects the existing
  one from a fresh controller start).

**Explicitly out of scope** (Phase 4): config knobs (sweep interval, enable/
scope, height/motion/palette), packaging/release, Kitty sprites. Also out of
scope: auto-starting the controller on session start (Spike B: no such trigger —
the user launches `control`, e.g. via the manifest action).

## 2. The pivotal constraint (Phase 3 spike, verified live 2026-07-23)

`layout.apply` (Phase 2's `place`) **re-materialises every pane and kills its
process** — a marker `sleep` was SIGHUP-killed by an injection. So it can never
be used for **automatic** injection. The safe primitive is an incremental
**`pane split --direction down --ratio R`**, which **preserves** the process, but
is **full-width only on a single-pane tab** (on a multi-pane tab it splits one
column). Hence: **auto-injection is non-destructive and scoped to single-pane
tabs** (covers all new tabs). See GOAL.md "Injection must never disturb running
work" and `docs/decisions.md`.

## 3. Grounding facts (verified live, herdr 0.7.0, 2026-07-23)

- `herdr tab list` → `result.tabs[]` each with `tab_id`, `pane_count`, `label`,
  `workspace_id`, `focused`.
- `herdr pane list` → `result.panes[]` each with `pane_id`, `tab_id`,
  `workspace_id`, and an **optional `label`** (present only when set — e.g. the
  Phase 1 pane shows `"Pets"`). This is the cheap de-dup signal.
- `herdr pane layout --pane <id>` → `result.layout.area.height` = tab rows (the
  `slim_ratio` denominator, reused from Phase 2).
- `herdr pane split <pane> --direction down --ratio R` → non-destructive; returns
  the new pane's `result.pane.pane_id`. On a single-pane tab the top child gets
  fraction `R`, the bottom (new) child gets `1-R`, both full tab width.
- `herdr pane run <pane> "<cmd>"` → runs `<cmd>` in that pane's shell.
- `herdr pane rename <pane> <label>` → sets the pane label (our de-dup marker).
- Spike B: `tab.created` events fire, but a periodic **poll** of `tab list` also
  reliably detects new tabs and is what we use (§4.3).

## 4. Architecture

One new module `control.rs`, a small `lock.rs`, one `main.rs` arm, one manifest
action. Reuses `HerdrCli` (all control ops are `herdr …` shell-outs — no new
transport) and `place::slim_ratio` / `place::TARGET_ROWS`.

| Unit | Responsibility | Depends on |
|---|---|---|
| `control.rs` (new) | Pure sweep logic + thin orchestration: parse tabs/panes, decide eligibility & de-dup, inject via split+run+rename, the sweep loop. | `herdr`, `place`, `lock` |
| `lock.rs` (new) | Single-owner advisory lock on a session-scoped path (`flock`); `acquire() -> Option<LockGuard>`. | — |
| `main.rs` (extend) | Dispatch `control`. | `control` |
| `herdr-plugin.toml` (extend) | `[[actions]]` `start-pets-controller` → `control`. | — |

### 4.1 Pure logic (unit-tested)

- `struct TabRef { tab_id: String, pane_count: u32 }`.
- `parse_tabs(list_json: &str) -> io::Result<Vec<TabRef>>` — from `herdr tab list`.
- `const STRIP_LABEL: &str = "herdr-pets"` and
  `fn is_strip_label(label: &str) -> bool` — true for `STRIP_LABEL` or `"Pets"`
  (so `place`/manual strips are also recognised, avoiding a double strip).
- `fn tabs_with_strip(panes_json: &str) -> io::Result<HashSet<String>>` — set of
  `tab_id`s that already contain a pane whose `label` satisfies `is_strip_label`
  (from one `herdr pane list`).
- `fn needs_strip(tab: &TabRef, has_strip: bool) -> bool` — `pane_count == 1 &&
  !has_strip`. (Single-pane ⇒ non-destructive full-width; excludes the strip's
  own now-2-pane tab on later sweeps, and multi-pane tabs.)
- `fn plan_injections(tabs: &[TabRef], with_strip: &HashSet<String>) ->
  Vec<String>` — the tab_ids to inject this sweep.

### 4.2 Injection (thin I/O over `HerdrCli`, one helper, live-verified)

`fn inject_strip(cli, tab_id, root_pane_id, self_exe) -> io::Result<()>`:
1. `pane layout --pane <root_pane_id>` → `area.height` = rows.
2. `ratio = slim_ratio(rows, TARGET_ROWS)`.
3. `pane split <root_pane_id> --direction down --ratio <ratio> --no-focus` →
   new `strip_pane_id`.
4. `pane run <strip_pane_id> "<self_exe> render"`.
5. `pane rename <strip_pane_id> <STRIP_LABEL>`.

The controller resolves each eligible tab's single pane id from the same
`pane list`. Any step failing on one tab is logged and skipped — one bad tab must
never abort the sweep (unobtrusive).

### 4.3 Sweep loop (orchestration, live-verified)

`fn control(cli, self_exe, interval_ms) -> io::Result<()>`:
1. `lock::acquire()` — if `None` (another controller owns it), print a line and
   exit `Ok(())`.
2. Loop: `tab list` + `pane list` → `with_strip = tabs_with_strip` →
   `plan_injections` → `inject_strip` each → sleep `interval_ms` (default 3000).

This single poll loop unifies **startup sweep**, **new-tab injection**, and
**respawn/re-assert** (a closed strip ⇒ tab has no strip next sweep ⇒ re-injected
if still eligible). Chosen over socket event subscription because it is simpler,
covers respawn in the same mechanism, and Spike B verified polling is reliable
(decision logged).

### 4.4 Lock

`lock.rs`: open/create a file at a session-scoped path (`$HERDR_SOCKET_PATH`'s
parent dir + `/herdr-pets-controller-<hash-of-full-socket-path>.lock`, else temp
dir — the hash keeps the lock **per session** so sessions sharing a socket
directory don't collide; whole-branch-review fix),
`flock(LOCK_EX | LOCK_NB)`. Hold the fd for process lifetime; the OS releases it
on exit (covers crashes). `acquire() -> io::Result<Option<LockGuard>>`; `None`
when already locked.

## 5. Data flow

```
herdr-pets control
  └─ lock::acquire() ── already held? ─▶ exit Ok
  └─ loop every interval:
       herdr tab list ─▶ TabRef[]
       herdr pane list ─▶ tabs_with_strip (label marker)
       plan_injections(tabs, with_strip)  [pure]
        └─ for each eligible single-pane tab:
             pane layout ─▶ rows ─▶ slim_ratio
             pane split down --ratio ─▶ strip pane
             pane run "<self_exe> render"
             pane rename <strip> herdr-pets
       sleep(interval)
```

## 6. Error handling

- Per-tab injection failures are logged (stderr) and skipped — the sweep and the
  other tabs continue (`rust-error-handling`: degrade at the boundary).
- Malformed `tab list` / `pane list` JSON: tolerant parse; a failed *whole-sweep*
  fetch logs and retries next interval rather than exiting.
- No `$HERDR_SOCKET_PATH`/CLI available: `control` still runs on the CLI seam
  (`herdr` shell-outs); if `herdr` itself fails, errors surface per-sweep.
- Lock contention is a normal, non-error outcome (clean exit).

## 7. Testing (TDD — failing test first)

Pure logic (fake `HerdrCli` returning canned JSON, per `rust-testability-seams`):
- `parse_tabs`: envelope → `TabRef`s (tab_id, pane_count).
- `is_strip_label` / `tabs_with_strip`: a pane labelled `herdr-pets` or `Pets`
  marks its tab; unlabelled/other panes don't.
- `needs_strip` / `plan_injections`: single-pane-without-strip is chosen;
  multi-pane and already-stripped tabs are excluded.
- `inject_strip`: a recording `HerdrCli` fake asserts the exact call sequence and
  args (`pane layout`, `pane split … --direction down --ratio …`, `pane run …
  render`, `pane rename … herdr-pets`), and that a `pane split` error aborts that
  tab without a `pane run`.
- `lock`: two `acquire()` on the same temp path — first `Some`, second `None`;
  dropping the first frees it.
- One sweep iteration (`sweep_once`) over a fake driven by canned JSON: asserts
  it injects exactly the eligible tabs and skips the rest.

Live verification (experiments, Task = last): a real `control` run in a scratch
workspace — new single-pane tab gets a slim full-width strip; a marker process in
that pane survives; closing the strip brings it back next sweep; a second
`control` exits on the lock; a multi-pane tab is left untouched.

## 8. Manifest

```toml
[[actions]]
id = "start-pets-controller"
title = "Start pets controller"
command = ["./target/release/herdr-pets", "control"]
```

## 9. Guardrails

- Branch off the Phase 2 tip (stacked; harness blocks merge to main — see
  decisions.md); never commit to `main`.
- Non-destructive injection ONLY — never `layout.apply` in the controller.
- Keep Phase 3 scoped — no config knobs (Phase 4).
- Gate green before done: `cargo test && cargo clippy --all-targets -- -D warnings
  && cargo fmt --check`.
