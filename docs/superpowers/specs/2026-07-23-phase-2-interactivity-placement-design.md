# Phase 2 — Interactivity & placement (design)

**Date:** 2026-07-23
**Phase:** 2 of 5 (see [`docs/PLAN.md`](../../PLAN.md))
**Status:** approved, pre-implementation
**Resolves against:** [`GOAL.md`](../../../GOAL.md), Phase 0 Spike A findings
([design §5](2026-07-23-phase-0-foundations-design.md))

## 1. Goal & exit criteria

A full-width pet strip can be placed beneath a multi-pane tab **on demand**, and
the pets are **interactive**: hovering a pet shows its agent's name in a caption
line, clicking a pet focuses that agent.

**Exit criteria:**
- `herdr-pets place` (and an equivalent herdr `[[actions]]` entry) turns the
  current tab into `[existing tree] / [full-width pets strip]`, where the strip
  is a new command pane running `herdr-pets render` as the full-width bottom
  child of a root vertical split.
- The strip lands at a **slim, fixed height** (≈7 rows) regardless of terminal
  size.
- Hovering a pet shows that agent's name in a caption line; moving off clears it.
- Clicking a pet runs `agent focus <terminal_id>` and herdr jumps to that agent.

**Explicitly out of scope** (Phases 3–4): auto-injection into every tab, the
new-tab event hook / polling sweep, respawn-on-close, strip de-duplication,
single-owner lockfile (all **Phase 3**); config knobs for height/motion/palette
(**Phase 4**).

## 2. What Phase 2 is (and is not)

Phase 2 delivers the **mechanism** — place a full-width strip into *one* tab, on
demand — plus mouse interactivity. Phase 3 layers the **automation** (every tab,
always, self-healing) on top of this same `place` core. Keeping placement a
plain, re-runnable operation now is what lets Phase 3's watchdog reuse it.

## 3. Grounding facts (verified live, 2026-07-23, herdr 0.7.0)

- **Env context.** herdr injects `HERDR_TAB_ID`, `HERDR_PANE_ID`,
  `HERDR_WORKSPACE_ID`, `HERDR_SOCKET_PATH` into every pane. `place` reads
  `$HERDR_TAB_ID` to know which tab to wrap — no discovery query needed.
- **Tab geometry.** `pane.edges` (CLI `herdr pane edges --current`) returns
  `result.edges.layout.area.height` — the tab's **total row count** (e.g. 64).
  This is the denominator for the slim-height ratio. Neither `tab.list` nor
  `layout.export` carries dimensions, so `pane.edges` is the source of truth.
- **Placement mechanism (Spike A).** `layout.export` + `layout.apply` over the
  raw socket is the *only* way to get a full-width bottom strip on a multi-pane
  tab; CLI `pane split`/`pane move` cannot (see Phase 0 design §5). The socket
  speaks newline-delimited JSON-RPC with dotted method names; `socket.rs`
  already exposes a one-shot `request()` helper for this.
- **`layout.apply` shape.** `params: { tab_id, root }` (`tab_id` XOR
  `workspace_id`, never both). A leaf with a `command` and **no `pane_id`**
  spawns a new command pane; a leaf with `pane_id` references an existing pane.
  Apply **rebuilds the tab** (new pane ids, new tab id) — a caveat for Phase 3's
  watchdog, but irrelevant to this one-shot injector which caches no ids across
  the apply.
- **Focus.** `herdr agent focus <target>` accepts terminal ids (also socket
  `agent.focus`). Pets already carry `terminal_id`, so click→focus routes
  through the existing CLI seam.

## 4. Architecture

One new module, targeted extensions to two existing ones, one data-shape change,
one manifest entry. Chosen over folding placement into `socket.rs` (muddles
transport with layout policy) and over a self-placing `render` pane (Spike A:
`layout.apply` rebuilds the tab, so a pane that placed itself would kill itself
mid-run).

| Unit | Responsibility | Depends on |
|---|---|---|
| `place.rs` (new) | The injector. Read `$HERDR_TAB_ID`, fetch tab rows via `pane.edges`, `layout.export` the tree, wrap it, `layout.apply`. Pure tree/ratio functions with thin socket I/O at the edges. | `socket`, `herdr` |
| `render.rs` (extend) | Mouse capture; hover hit-testing → caption line; click → `agent focus`. | `herd`, `herdr`, `agent` |
| `herd.rs` / `pet.rs` (extend) | `Pet` carries a display `label`, set/updated in `reconcile`. | `agent` |
| `main.rs` (extend) | Dispatch the new `place` subcommand. | `place` |
| `herdr-plugin.toml` (extend) | `[[actions]]` entry invoking `place`. | — |

### 4.1 `place.rs` — the injector

Flow:
1. Resolve `tab_id` from `$HERDR_TAB_ID` (error out clearly if unset — `place`
   only makes sense inside a herdr session).
2. `pane.edges` → `layout.area.height` = `tab_rows`.
3. `layout.export` (params `{tab_id}`) → the current tree (`result.layout.root`).
4. `root = wrap_root(exported_tree, slim_ratio(tab_rows), cmd, cwd)`.
5. `layout.apply` with `{tab_id, root}`.

Pure, unit-testable functions (the socket I/O stays a thin shell around them):

- `wrap_root(tree, ratio, cmd, cwd) -> Root`: returns a `down` split with
  `ratio`, `first` = `tree` verbatim, `second` = a new command-pane leaf
  `{ type: "pane", command: cmd, cwd }` (**no `pane_id`** → spawns fresh). `cmd`
  is the absolute path to the built `herdr-pets` binary plus `render`.
- `slim_ratio(tab_rows, target_rows) -> f32` = `1 - target_rows / tab_rows`,
  with `target_rows` defaulting to `7` and the result clamped to a sane floor
  (e.g. `0.3`) so a tiny tab still leaves a usable top region.

De-duplication (avoid stacking a second strip if one already exists in the tree)
is **deferred to Phase 3** per the plan — Phase 2 wraps whatever tree it exports.

### 4.2 `render.rs` — interactivity

- **Mouse capture.** `EnableMouseCapture` on setup, `DisableMouseCapture` on the
  terminal-restore path (alongside the existing raw-mode / alternate-screen
  teardown), so the terminal is always left clean.
- **Hit-testing.** A pure fn `pet_at_column(herd, species, col) -> Option<usize>`:
  map the mouse column directly to a pixel x (half-block rendering is 1 cell = 1
  pixel horizontally), find pets whose drawn column span contains it, and return
  the **topmost** (highest `priority`, matching the draw z-order in `draw_herd`).
  Gaps and out-of-range columns return `None`.
- **Hover.** On `Mouse(Moved)`, recompute `hovered: Option<String>` (the hit
  pet's `terminal_id`).
- **Caption line.** Reserve the strip's **bottom row**. Draw the hovered pet's
  `label` there; blank when nothing is hovered → the layout never reflows. The
  herd draws into `area` minus that last row. If the pane is shorter than pets +
  caption, the caption is simply clipped (acceptable degradation).
- **Click.** On `Mouse(Down(Left))`, if `pet_at_column` hits, shell
  `herdr agent focus <terminal_id>` through the existing `HerdrCli` /
  `CommandRunner` seam. A focus failure is logged and swallowed — it must never
  crash or interrupt the strip (per the "unobtrusive / never steals work" goal).

### 4.3 Data change

`Pet` gains `label: String`. `Herd::reconcile` sets it from `Agent::label()` for
new pets and refreshes it for survivors (an agent can be renamed mid-session).
This is the only field the hover caption needs; positions/status/identity are
unchanged.

### 4.4 Manifest

Add alongside the existing `[[panes]]`:

```toml
[[actions]]
id = "place-pets"
title = "Place pets strip"
command = ["./target/release/herdr-pets", "place"]
```

The existing `[[panes]]` entry (manual `plugin pane open`) stays — useful for dev
and as the pane the injected strip runs.

## 5. Data flow

```
user runs `place` (CLI or herdr action, from any pane in the tab)
      │  reads $HERDR_TAB_ID
      ▼
place.rs ── pane.edges ─▶ tab_rows
         ── layout.export ─▶ current tree
         ── wrap_root(tree, slim_ratio(tab_rows)) ─▶ root
         ── layout.apply {tab_id, root}
      ▼
herdr spawns the full-width bottom pane → runs `herdr-pets render`
      ▼
render loop: watcher feeds agent snapshots → herd (pets carry label)
      │
      ├─ Mouse(Moved) → pet_at_column → hovered → caption line
      └─ Mouse(Down Left) → pet_at_column → `agent focus <terminal_id>`
```

## 6. Error handling

- `place`: missing `$HERDR_TAB_ID`, a socket connect failure, or an
  `error`-envelope reply from `pane.edges`/`layout.export`/`layout.apply` all
  surface as a clear `io::Error` and a non-zero exit (per `rust-error-handling`).
  No partial-apply recovery is needed — a failed `apply` leaves the tab
  untouched.
- `render` click: `agent focus` failures degrade silently (log-only); the strip
  keeps running.
- Malformed `pane.edges` / `layout.export` JSON: tolerant parsing
  (`rust-serde-tolerant-parsing`); on parse failure `place` errors out rather
  than applying a malformed tree.

## 7. Testing (TDD — failing test first)

- **`place.rs`**
  - `wrap_root`: resulting root is a `down` split; `first` equals the input tree;
    `second` is a leaf with the expected `command`/`cwd` and **no `pane_id`**.
  - `slim_ratio`: representative `tab_rows` → expected ratio; clamps on tiny tabs.
- **`render.rs`**
  - `pet_at_column`: overlapping pets → topmost by priority; a gap → `None`;
    out-of-range column → `None`.
  - Snapshot: strip with one pet hovered shows its name in the bottom caption row
    (and a not-hovered snapshot shows a blank caption row).
- **`herd.rs`**: `reconcile` populates `label` for new pets and updates it for a
  renamed survivor.
- **Click→focus**: a fake `CommandRunner` records the args; assert it receives
  `["agent", "focus", "<terminal_id>"]` on a left-click over a pet, and nothing
  on a click over empty space.

Snapshot rendering follows `rust-tui-snapshot-testing` (insta + `TestBackend`,
deterministic frozen positions).

## 8. Verification (experiments, not unit tests)

1. **Mouse forwarding (first implementation step).** Confirm herdr forwards
   mouse events into a plugin pane (build, open the pane, move/click, observe).
   Low-risk — panes are full PTYs — but it gates all of §4.2, so verify before
   building hover/click. If it fails, flag it and revisit (per the handoff
   guardrails).
2. **`place` in an isolated scratch tab** with 2+ panes: run `herdr-pets place`,
   confirm the strip lands full-width across the bottom at ≈7 rows and runs the
   renderer; then confirm the `[[actions]]` invocation inherits the focused
   tab's `$HERDR_TAB_ID` and behaves identically. Run in a scratch tab so any
   layout mishap is isolated from the live session.

## 9. Guardrails (from the handoff)

- Work on a branch off `main` (already on `feature/phase-2`); never commit to
  `main` directly.
- **Do not commit or push without the user asking**; local checkpoint commits may
  be proposed.
- Keep Phase 2 scoped — no drift into Phase 3 automation or Phase 4 config.
- If verification contradicts the design (e.g. mouse events don't reach the
  pane), update `GOAL.md` + `docs/PLAN.md` and flag it before proceeding.
