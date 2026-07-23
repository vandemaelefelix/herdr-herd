# Phase 4 — Config & Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make herdr-pets configurable (a small TOML config), reduced-motion-aware, documented, and CI-gated.

**Architecture:** A new `config.rs` hand-parses four flat keys from `config.toml` in the plugin config dir (no new crate dependency), degrading to opinionated defaults. The `control` watchdog honors `enabled`/`sweep_interval_ms`/`strip_rows`; the `render` strip honors `reduced_motion` by skipping the roam step. Plus README docs and a CI workflow.

**Tech Stack:** Rust (edition 2024), std only. No new crate dependencies.

## Global Constraints

- Rust **edition 2024**, `rust-version = 1.96`.
- **No `unwrap`/`expect` outside `#[cfg(test)]`.** `io::Result` + `?`; `io::Error::other`.
- **No new crate dependencies.** Config is parsed by a hand-rolled reader (`toml` stays a dev-dependency only).
- Branch **`feat/phase-4-config-polish`** (already checked out, stacked on the Phase 3 tip); never commit to `main`.
- **TDD:** failing test first, watch it fail, minimal implementation, watch it pass, commit.
- Sentence-style test names; `//!` module headers; `///` on public items.
- Scope: the four knobs (`enabled`, `strip_rows`, `sweep_interval_ms`, `reduced_motion`), docs, CI. **No** packaging artifacts, **no** Kitty sprites, **no** speculative knobs.
- Keep the gate green each task: `cargo fmt --check` + `cargo clippy -p herdr-pets --all-targets -- -D warnings` + `cargo test -p herdr-pets`.

## File Structure

- `src/config.rs` (create) — `Config`, `from_toml_str`, `load_from_dir`, `resolve_config_dir`, `load`.
- `src/lib.rs` (modify) — register `pub mod config;`.
- `src/control.rs` (modify) — thread `target_rows` into `inject_strip`/`sweep_once`/`control`.
- `src/render.rs` (modify) — `run`/`run_loop` gain `reduced_motion`; skip `herd.step` when set.
- `src/main.rs` (modify) — load config in `render` + `control` arms; honor `enabled`.
- `README.md` (modify) — usage + config docs.
- `.github/workflows/ci.yml` (create) — the gate on push/PR.

---

## Task 1: The config module (`config.rs`)

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs` (register)

**Interfaces:**
- Produces: `pub struct Config { pub enabled: bool, pub strip_rows: u16, pub sweep_interval_ms: u64, pub reduced_motion: bool }` (with `Default`); `Config::from_toml_str(&str) -> Config`; `load_from_dir(&Path) -> Config`; `resolve_config_dir() -> Option<PathBuf>`; `load() -> Config`.

- [ ] **Step 1: Register the module** — add to `src/lib.rs`, alphabetical (between `config`… it sorts after `anim`/`control`? order: agent, anim, config, control, herd — `config` < `control`). Insert `pub mod config;` right after `pub mod anim;` and before `pub mod control;`.

- [ ] **Step 2: Create `src/config.rs` with the module header, uses, and failing tests first:**

```rust
//! Plugin configuration: a tiny, tolerant reader for the four opinionated knobs.
//! Parsed by hand (no new crate dependency) from `config.toml` in the plugin
//! config dir; any missing or malformed key degrades to its default.

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_the_opinionated_values() {
        assert_eq!(
            Config::default(),
            Config { enabled: true, strip_rows: 7, sweep_interval_ms: 3000, reduced_motion: false }
        );
    }

    #[test]
    fn from_toml_str_parses_all_four_keys() {
        let c = Config::from_toml_str(
            "enabled = false\nstrip_rows = 5\nsweep_interval_ms = 1500\nreduced_motion = true\n",
        );
        assert_eq!(
            c,
            Config { enabled: false, strip_rows: 5, sweep_interval_ms: 1500, reduced_motion: true }
        );
    }

    #[test]
    fn from_toml_str_defaults_missing_keys_and_ignores_comments() {
        let c = Config::from_toml_str("# a comment\nreduced_motion = true  # calm\n");
        assert!(c.reduced_motion);
        assert!(c.enabled, "an unspecified key keeps its default");
        assert_eq!(c.strip_rows, 7);
    }

    #[test]
    fn from_toml_str_ignores_malformed_lines_and_unknown_keys() {
        let c = Config::from_toml_str(
            "garbage line\nunknown_key = 9\nstrip_rows = notanumber\nenabled = true\n",
        );
        assert_eq!(c, Config::default(), "malformed/unknown ignored; enabled=true matches default");
    }

    #[test]
    fn load_from_dir_reads_config_toml_when_present() {
        let dir = std::env::temp_dir().join(format!("herdr-pets-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "strip_rows = 9\n").unwrap();
        assert_eq!(load_from_dir(&dir).strip_rows, 9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_from_dir_defaults_when_the_file_is_absent() {
        let dir = std::env::temp_dir().join(format!("herdr-pets-cfg-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_from_dir(&dir), Config::default());
    }
}
```

- [ ] **Step 3: Run tests to verify they fail (do not compile)**

Run: `cargo test -p herdr-pets --lib config::`
Expected: FAIL — `cannot find type 'Config'` / `cannot find function 'from_toml_str'`.

- [ ] **Step 4: Implement** — insert above the `#[cfg(test)]` module:

```rust
/// The four opinionated knobs. Sensible defaults; a config file overrides them.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Whether the `control` watchdog runs at all.
    pub enabled: bool,
    /// Strip height in rows.
    pub strip_rows: u16,
    /// Controller poll cadence in milliseconds.
    pub sweep_interval_ms: u64,
    /// Calm pets — no wandering or bounce.
    pub reduced_motion: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config { enabled: true, strip_rows: 7, sweep_interval_ms: 3000, reduced_motion: false }
    }
}

impl Config {
    /// Parse a `config.toml` body: start from defaults and override recognized
    /// keys. Tolerant — comments (`#`), blank lines, unknown keys, and
    /// unparsable values are ignored, so a malformed config degrades to defaults
    /// rather than crashing.
    pub fn from_toml_str(s: &str) -> Config {
        let mut cfg = Config::default();
        for raw in s.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, val)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let val = val.trim().trim_matches(['"', '\'']).trim();
            match key {
                "enabled" => {
                    if let Ok(v) = val.parse() {
                        cfg.enabled = v;
                    }
                }
                "strip_rows" => {
                    if let Ok(v) = val.parse() {
                        cfg.strip_rows = v;
                    }
                }
                "sweep_interval_ms" => {
                    if let Ok(v) = val.parse() {
                        cfg.sweep_interval_ms = v;
                    }
                }
                "reduced_motion" => {
                    if let Ok(v) = val.parse() {
                        cfg.reduced_motion = v;
                    }
                }
                _ => {}
            }
        }
        cfg
    }
}

/// Read `dir/config.toml` if present; otherwise return defaults.
pub fn load_from_dir(dir: &Path) -> Config {
    match std::fs::read_to_string(dir.join("config.toml")) {
        Ok(s) => Config::from_toml_str(&s),
        Err(_) => Config::default(),
    }
}

/// Resolve the plugin config dir by asking herdr (`herdr plugin config-dir
/// herdr-pets`, plain-path stdout). Thin glue; `None` on any failure.
pub fn resolve_config_dir() -> Option<PathBuf> {
    let bin = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string());
    let out = Command::new(bin)
        .args(["plugin", "config-dir", "herdr-pets"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?;
    let path = path.trim();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

/// The effective config: from the resolved config dir, or defaults.
pub fn load() -> Config {
    resolve_config_dir()
        .map(|d| load_from_dir(&d))
        .unwrap_or_default()
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p herdr-pets --lib config::`
Expected: PASS (all six).

- [ ] **Step 6: Format, lint, commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy -p herdr-pets --all-targets -- -D warnings`
Expected: clean.

```bash
git add src/config.rs src/lib.rs
git commit -m "feat(config): tolerant config reader for the four opinionated knobs"
```

---

## Task 2: Wire config into the controller

**Files:**
- Modify: `src/control.rs` (`inject_strip`, `sweep_once`, `control` gain `target_rows`; update their tests)
- Modify: `src/main.rs` (`control` arm loads config, honors `enabled`, passes interval + strip_rows)

**Interfaces:**
- Consumes: `crate::config::Config`.
- Produces (changed signatures):
  - `pub fn inject_strip(cli, root_pane_id, self_exe, target_rows: u16) -> io::Result<()>`
  - `pub fn sweep_once(cli, self_exe, target_rows: u16) -> io::Result<()>`
  - `pub fn control(cli, self_exe, lock_path, interval, target_rows: u16) -> io::Result<()>`

- [ ] **Step 1: Update the `inject_strip` test for the new arg** — in `src/control.rs`, in `inject_strip_splits_runs_and_labels_in_order`, change the call to pass `7` and keep the `"0.8906"` assertion (slim_ratio(64, 7)):

```rust
        inject_strip(&cli, "w1:p1", "/abs/herdr-pets", 7).unwrap();
```

And in `inject_strip_aborts_the_tab_without_running_or_renaming_when_split_fails`, change its call similarly:

```rust
        let err = inject_strip(&cli, "w1:p1", "/abs/herdr-pets", 7);
```

(Keep the rest of both tests as-is.)

- [ ] **Step 2: Update the `sweep_once` tests for the new arg** — in `sweep_once_injects_only_the_eligible_tab` and `sweep_once_continues_after_one_tab_fails`, change the calls to pass `7`:

```rust
        sweep_once(&cli, "/abs/herdr-pets", 7).unwrap();
```

- [ ] **Step 3: Run tests to verify they fail (do not compile)**

Run: `cargo test -p herdr-pets --lib control::`
Expected: FAIL — arity mismatch (`inject_strip`/`sweep_once` take 3/2 args, called with 4/3).

- [ ] **Step 4: Thread `target_rows`** — in `src/control.rs`:

In `inject_strip`, replace the signature and the ratio line:

```rust
pub fn inject_strip(
    cli: &dyn HerdrCli,
    root_pane_id: &str,
    self_exe: &str,
    target_rows: u16,
) -> io::Result<()> {
    let layout_json = cli.run_json(&["pane", "layout", "--pane", root_pane_id])?;
    let rows = parse_tab_rows(&layout_json)?;
    let ratio_arg = format!("{:.4}", slim_ratio(rows, target_rows));
```

(Leave the rest of `inject_strip` unchanged. Remove the now-unused `TARGET_ROWS` import if it becomes unused: change the `use crate::place::{TARGET_ROWS, parse_tab_rows, slim_ratio};` to `use crate::place::{parse_tab_rows, slim_ratio};`.)

In `sweep_once`, thread it through:

```rust
pub fn sweep_once(cli: &dyn HerdrCli, self_exe: &str, target_rows: u16) -> io::Result<()> {
    let tabs = parse_tabs(&cli.run_json(&["tab", "list"])?)?;
    let panes = parse_panes(&cli.run_json(&["pane", "list"])?)?;
    for (tab_id, root_pane) in plan_injections(&tabs, &panes) {
        if let Err(e) = inject_strip(cli, &root_pane, self_exe, target_rows) {
            eprintln!("herdr-pets: could not place strip in {tab_id}: {e}");
        }
    }
    Ok(())
}
```

In `control`, add the param and pass it:

```rust
pub fn control(
    cli: &dyn HerdrCli,
    self_exe: &str,
    lock_path: &Path,
    interval: Duration,
    target_rows: u16,
) -> io::Result<()> {
    let _guard = match lock::acquire(lock_path)? {
        Some(g) => g,
        None => {
            eprintln!("herdr-pets: another controller is already running; exiting");
            return Ok(());
        }
    };
    loop {
        if let Err(e) = sweep_once(cli, self_exe, target_rows) {
            eprintln!("herdr-pets: sweep failed: {e}");
        }
        std::thread::sleep(interval);
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p herdr-pets --lib control::`
Expected: PASS.

- [ ] **Step 6: Load config + honor `enabled` in `main.rs`** — replace the `Some("control")` arm in `src/main.rs`:

```rust
        Some("control") => {
            let cfg = herdr_pets::config::load();
            if !cfg.enabled {
                eprintln!("herdr-pets: disabled by config; not starting the controller");
                return ExitCode::SUCCESS;
            }
            let cli = LiveHerdr::from_env();
            let self_exe = std::env::current_exe()
                .ok()
                .and_then(|p| p.to_str().map(String::from))
                .unwrap_or_else(|| "herdr-pets".to_string());
            let lock_path = herdr_pets::control::controller_lock_path();
            match herdr_pets::control::control(
                &cli,
                &self_exe,
                &lock_path,
                std::time::Duration::from_millis(cfg.sweep_interval_ms),
                cfg.strip_rows,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("herdr-pets: {e}");
                    ExitCode::FAILURE
                }
            }
        }
```

- [ ] **Step 7: Build and run the full suite**

Run: `cargo build -p herdr-pets && cargo test -p herdr-pets`
Expected: PASS.

- [ ] **Step 8: Format, lint, commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy -p herdr-pets --all-targets -- -D warnings`
Expected: clean.

```bash
git add src/control.rs src/main.rs
git commit -m "feat(control): honor config enabled, sweep interval, and strip height"
```

---

## Task 3: Reduced-motion in the renderer

**Files:**
- Modify: `src/render.rs` (`run` + `run_loop` gain `reduced_motion`; skip `herd.step` when set)
- Modify: `src/main.rs` (`render` arm loads config, passes `reduced_motion`)

**Interfaces:**
- Produces (changed): `pub fn run(rx, species, theme, focus, reduced_motion: bool) -> io::Result<()>`.

- [ ] **Step 1: Add `reduced_motion` to `run`** — in `src/render.rs`, update `run`'s signature and its `run_loop` call:

```rust
pub fn run(
    rx: Receiver<Vec<Agent>>,
    species: Vec<Species>,
    theme: Theme,
    focus: Box<dyn HerdrCli>,
    reduced_motion: bool,
) -> io::Result<()> {
```

Find the `run_loop(...)` call inside `run` and add `reduced_motion` as its final argument.

- [ ] **Step 2: Add `reduced_motion` to `run_loop` and gate the step** — update `run_loop`'s signature (add `reduced_motion: bool` as the final parameter), then wrap the existing `herd.step(...)` call so it is skipped when reduced motion is on:

```rust
        if !reduced_motion {
            herd.step(dt_ms, w, pet_w, &mut rng);
        }
```

(Use the exact variable names already present at the existing `herd.step(...)` call site — do not rename them. Everything else in the loop, including `draw_herd`/`draw_caption` and the mouse handling, stays unchanged. With the step skipped, pets keep `phase = 0.0`, so their per-frame `motion_offset` stays ~0 and they render calm.)

- [ ] **Step 3: Pass `reduced_motion` from `main.rs`** — in `src/main.rs`, update the `Some("render")` arm to load config and pass the flag:

```rust
        Some("render") => {
            let cfg = herdr_pets::config::load();
            let species = load_species();
            let (tx, rx) = mpsc::channel();
            let cli = Box::new(LiveHerdr::from_env());
            let focus = Box::new(LiveHerdr::from_env());
            let socket: Option<Box<dyn SocketClient + Send>> = socket_path()
                .and_then(|p| RealSocket::connect(&p).ok())
                .map(|s| Box::new(s) as Box<dyn SocketClient + Send>);
            let _watcher = watch(cli, socket, Box::new(RealClock::new()), tx, 2500, 250);
            match render::run(rx, species, Theme::Dark, focus, cfg.reduced_motion) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("herdr-pets: {e}");
                    ExitCode::FAILURE
                }
            }
        }
```

- [ ] **Step 4: Build and run the full suite**

Run: `cargo build -p herdr-pets && cargo test -p herdr-pets`
Expected: PASS. (The reduced-motion loop behavior is verified live in Task 5; no unit test drives the terminal loop, consistent with how the mouse loop was handled in Phase 2.)

- [ ] **Step 5: Format, lint, commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy -p herdr-pets --all-targets -- -D warnings`
Expected: clean.

```bash
git add src/render.rs src/main.rs
git commit -m "feat(render): honor reduced_motion by freezing the roam step"
```

---

## Task 4: User docs + CI

**Files:**
- Modify: `README.md`
- Create: `.github/workflows/ci.yml`

**Interfaces:** none (docs + CI).

- [ ] **Step 1: Expand `README.md`** — read the current `README.md`, then add (or update) a "Usage" and "Configuration" section. It MUST document: (a) install via `herdr plugin link .` (dev) and `herdr plugin install <owner>/<repo>` (builds from source via the manifest `[[build]]`); (b) the `place-pets` action / `herdr-pets place` (on-demand full-width strip, uses the destructive rebuild so the user opts in); (c) the `start-pets-controller` action / `herdr-pets control` (the always-on watchdog; non-destructive; auto-injects into single-pane tabs); (d) the config file at `herdr plugin config-dir herdr-pets` → `config.toml`, with every key, its type, and its default:

```markdown
## Configuration

herdr-pets reads an optional `config.toml` from its plugin config dir
(`herdr plugin config-dir herdr-pets`). Every key is optional and falls back to
an opinionated default:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `true` | Whether the `control` watchdog runs. |
| `strip_rows` | int | `7` | Strip height, in rows. |
| `sweep_interval_ms` | int | `3000` | Controller poll cadence (ms). |
| `reduced_motion` | bool | `false` | Calm pets — no wandering or bounce. |

Example `config.toml`:

    reduced_motion = true
    strip_rows = 6
```

Keep the existing README content; append/integrate these sections rather than replacing the file.

- [ ] **Step 2: Create the CI workflow** — write `.github/workflows/ci.yml`:

```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:
jobs:
  gate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust 1.96
        run: |
          rustup toolchain install 1.96 --profile minimal --component clippy,rustfmt
          rustup default 1.96
      - name: Format
        run: cargo fmt --check
      - name: Clippy
        run: cargo clippy --all-targets -- -D warnings
      - name: Test
        run: cargo test
```

- [ ] **Step 3: Validate the workflow YAML parses**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('ci.yml: valid YAML')"`
Expected: `ci.yml: valid YAML`.

- [ ] **Step 4: Commit**

```bash
git add README.md .github/workflows/ci.yml
git commit -m "docs(phase-4): document usage + config; add CI gate workflow"
```

---

## Task 5: Live verification & wrap-up

- [ ] **Step 1: Build release + relink**

Run: `cargo build --release -p herdr-pets && herdr plugin link .`
Expected: builds; relinks cleanly.

- [ ] **Step 2: Verify config is read** — write a config and confirm the effective values:

```bash
CFGDIR="$(herdr plugin config-dir herdr-pets)"
mkdir -p "$CFGDIR"
printf 'reduced_motion = true\nstrip_rows = 5\n' > "$CFGDIR/config.toml"
```

In an isolated scratch single-pane tab, non-destructively inject a strip at the configured height by replicating the controller's injection (`herdr pane split <pane> --direction down --ratio <slim_ratio(rows,5)>` + `herdr pane run <new> "<abs>/target/release/herdr-pets render"`); confirm the strip is ~5 rows and pets are calm (not wandering). Remove the scratch config afterward (`rm "$CFGDIR/config.toml"`) so the environment is left clean.

- [ ] **Step 3: Verify `enabled = false`** — write `enabled = false` to the config, run `./target/release/herdr-pets control`, and confirm it prints the disabled message and exits SUCCESS without injecting. Remove the config afterward.

- [ ] **Step 4: Run the full gate**

Run: `cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check`
Expected: all pass, no warnings.

- [ ] **Step 5: Update the phase tracker** — in `docs/PLAN.md`, set the Phase 4 row to:

```markdown
| 4 | Config & polish | Done | [design](superpowers/specs/2026-07-23-phase-4-config-polish-design.md) | [plan](superpowers/plans/2026-07-23-phase-4-config-polish.md) |
```

- [ ] **Step 6: Commit**

```bash
git add docs/PLAN.md
git commit -m "docs(phase-4): mark Phase 4 done and link the plan"
```

---

## Self-Review

**Spec coverage:**
- §1/§4.1 config module (4 keys, defaults, tolerant parse, load) → Task 1.
- §4.2 control honors enabled/interval/strip_rows → Task 2.
- §4.2 render honors reduced_motion (skip step) → Task 3.
- §1 docs + CI → Task 4.
- §7 live verification → Task 5.
- §2 deferrals (packaging, Kitty, extra knobs) → not implemented, recorded in decisions.md.

**Placeholder scan:** No TBD/TODO. Every code step shows complete code; the README step names exactly what to document with the config table verbatim.

**Type consistency:** `Config { enabled: bool, strip_rows: u16, sweep_interval_ms: u64, reduced_motion: bool }` used identically across tasks. `inject_strip(.., target_rows: u16)`, `sweep_once(.., target_rows: u16)`, `control(.., target_rows: u16)` consistent (Task 2). `render::run(.., reduced_motion: bool)` threaded from main.rs (Task 3). `slim_ratio(64, 7)` → `"0.8906"` unchanged in the updated Task 2 test (default strip_rows = 7).
