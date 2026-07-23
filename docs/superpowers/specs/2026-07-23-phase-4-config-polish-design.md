# Phase 4 — Config & polish (design)

**Date:** 2026-07-23
**Phase:** 4 of 5 (see [`docs/PLAN.md`](../../PLAN.md))
**Status:** approved (autonomous run — brainstorming gate waived), pre-implementation
**Resolves against:** [`GOAL.md`](../../../GOAL.md) ("opinionated defaults, few knobs").

## 1. Goal & exit criteria

The plugin is **configurable, documented, and installable**: a small TOML config
controls the handful of knobs that matter, the renderer honors reduced-motion,
and there is user documentation plus CI.

**Exit criteria:**
- A `config.toml` in the plugin config dir controls: `enabled`, `strip_rows`,
  `sweep_interval_ms`, `reduced_motion`. Missing file / missing keys ⇒ sensible
  defaults (opinionated-defaults principle). A malformed file degrades to
  defaults rather than crashing.
- The `control` watchdog honors `enabled` (exit cleanly if off), `strip_rows`
  (strip height), and `sweep_interval_ms` (poll cadence).
- The `render` strip honors `reduced_motion` (pets are calm — no wander, no
  bounce).
- `README.md` documents install, the `place` action, the `control` watchdog +
  `start-pets-controller` action, and every config key with its default.
- CI (`.github/workflows/ci.yml`) runs the gate (`cargo test` + `clippy -D
  warnings` + `fmt --check`) on push/PR.

## 2. Scope (and deliberate deferrals)

Phase 4 in the roadmap is broad. To stay reviewable and honor YAGNI, Phase 4
delivers the **highest-value, opinionated** knobs + docs + CI. Explicitly
**deferred** (recorded in `docs/decisions.md`), because each is either large
release infrastructure or low-value polish that the mission's "few knobs"
principle argues against shipping speculatively:
- Prebuilt release binaries / `fetch-or-build` + a cut GitHub release. The
  existing `[[build]]` source-build already makes `herdr plugin install
  <owner>/<repo>` work; cutting the actual tag/release is a maintainer action
  after this PR stack merges to `main` (the harness blocks autonomous merges).
- `scope` filter (which agents), palette customization, and per-state behavior
  overrides — deferred until there's a real request; the deterministic identity
  + theme defaults already serve the mission.
- **Kitty-graphics sprites** — explicitly a *stretch* in the roadmap; the
  half-block renderer stays the universal default. Out of scope.

## 3. Grounding facts (verified live, herdr 0.7.0, 2026-07-23)

- The plugin config dir is `herdr plugin config-dir herdr-pets` →
  `<herdr-config>/plugins/config/herdr-pets` (plain-path stdout, exit 0). herdr
  injects no config-dir env var into panes (only `HERDR_ENV`, `HERDR_SOCKET_PATH`,
  `HERDR_{TAB,PANE,WORKSPACE}_ID`), so the plugin resolves the dir by shelling
  `herdr plugin config-dir herdr-pets`.
- Motion has two sources: wandering in `Herd::step` (updates `target_x`/`x`) and a
  per-frame bounce in `draw_herd` via `motion_offset(spec, pet.phase)`. Pets are
  created with `phase = 0.0`. Skipping `step` freezes both wander and phase, so
  `motion_offset(_, 0.0)` ≈ zero ⇒ calm pets **without touching `draw_herd` or
  its snapshots**.

## 4. Architecture

One new module `config.rs`; targeted wiring into `control.rs`, `render.rs`, and
`main.rs`; plus docs and CI files. **No new crate dependencies** — `toml` is only
a dev-dependency and the constraint forbids adding runtime crates, so config is
parsed by a tiny hand-rolled tolerant `key = value` reader (the four keys are
flat scalars, valid TOML syntax; the parser reads that subset).

| Unit | Responsibility | Depends on |
|---|---|---|
| `config.rs` (new) | `Config` struct + `Default`; `from_toml_str` (hand-rolled, tolerant); `load_from_dir(path)`; `resolve_config_dir()` (thin glue shelling `herdr plugin config-dir`). | std only |
| `control.rs` (extend) | Honor `enabled` (exit), `sweep_interval_ms`, `strip_rows` (thread into `inject_strip`). | config |
| `render.rs` (extend) | Honor `reduced_motion` — skip `Herd::step`. | config |
| `main.rs` (extend) | Load config in `render` + `control` arms; pass values in. | config |
| `README.md` (extend) | User docs. | — |
| `.github/workflows/ci.yml` (new) | Gate on CI. | — |

### 4.1 `config.rs`

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub enabled: bool,          // default true — controller active
    pub strip_rows: u16,        // default 7   — strip height
    pub sweep_interval_ms: u64, // default 3000 — controller poll cadence
    pub reduced_motion: bool,   // default false — calm pets
}
```
`impl Default` supplies the defaults. `from_toml_str(s) -> Config` starts from
`Config::default()` and overrides recognized keys: it scans lines, strips `#`
comments and whitespace, splits each `key = value` on the first `=`, and matches
the four known keys (parsing `true`/`false` for bools, integers for
`strip_rows`/`sweep_interval_ms`, trimming optional surrounding quotes). Unknown
keys and unparsable values are ignored (tolerant — a malformed config never
crashes the strip; it degrades to defaults per field). `load_from_dir(dir) ->
Config` reads `dir/config.toml` if present (else default), then `from_toml_str`.
`resolve_config_dir() -> Option<PathBuf>` shells `herdr plugin config-dir
herdr-pets` and trims the path (thin, untested glue).

### 4.2 Wiring

- **control**: `main.rs` loads `Config`; if `!enabled`, print a line and exit
  `SUCCESS` (never start the watchdog). Else pass `sweep_interval_ms` as the loop
  interval and `strip_rows` into the sweep. `inject_strip` gains a `target_rows:
  u16` param (replacing the hard-coded `TARGET_ROWS`); `sweep_once` threads it.
- **render**: `main.rs` loads `Config`; `render::run` gains `reduced_motion:
  bool`; `run_loop` skips `herd.step(...)` when it is set (pets stay calm). The
  on-demand `place` strip keeps the default height (7) — config-driven height is
  the always-on controller's concern; documented.

## 5. Error handling

- Missing config file, missing keys, or malformed TOML ⇒ `Config::default()`
  (degrade, never crash — `rust-error-handling`).
- `resolve_config_dir` failure (herdr CLI missing) ⇒ `None` ⇒ callers use
  `Config::default()`.

## 6. Testing (TDD — failing test first)

- `config.rs` (pure, unit-tested):
  - `from_toml_str` full config → all fields parsed.
  - partial config (only `reduced_motion = true`) → that field set, others
    defaulted.
  - empty string / malformed TOML → `Config::default()` (no panic).
  - `Config::default()` values are exactly `true/7/3000/false`.
  - `load_from_dir` on a temp dir with a written `config.toml` → parsed; on an
    empty temp dir → defaults.
- `inject_strip` test updated for the new `target_rows` arg (ratio reflects it).
- `render` reduced-motion: a focused test that stepping is skipped — assert pet
  positions are unchanged after a `run`-equivalent tick when `reduced_motion`
  (or a snapshot showing calm pets). Kept light; the wiring is verified live.
- Existing snapshots unchanged (draw path untouched).

## 7. Verification (live)

- Write a `config.toml` with `reduced_motion = true`, `strip_rows = 5`; open a
  strip and confirm calm pets at ~5 rows.
- `enabled = false` ⇒ `control` exits immediately.
- `README` renders; CI workflow file is valid YAML and runs the gate.

## 8. Guardrails

- Branch off the Phase 3 tip (stacked; harness blocks merge to main — see
  decisions.md); never commit to `main`.
- Keep it scoped — the four knobs above, docs, CI. No packaging artifacts, no
  Kitty sprites, no speculative knobs.
- Gate green before done: `cargo test && cargo clippy --all-targets -- -D
  warnings && cargo fmt --check`.
