# Phase 3 — Controller / Watchdog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A long-lived `control` watchdog that keeps a slim, full-width pet strip in every eligible tab — appearing in new tabs, returning if closed — without ever killing a running process.

**Architecture:** A poll-based sweep. Each interval the controller lists tabs + panes, decides (pure logic) which single-pane tabs lack a strip, and injects one via a **non-destructive** `pane split` (never `layout.apply`, which kills processes). A single-owner `flock` guarantees one controller; strips are de-duped by a pane label marker so restarts/session-restore never stack a second strip.

**Tech Stack:** Rust (edition 2024). `std::fs::File::try_lock`/`TryLockError` (stable ≥1.89) for the lock — **no new crate dependencies**. Reuses `HerdrCli` (shell-out seam) and `place::{slim_ratio, TARGET_ROWS, parse_tab_rows}`.

## Global Constraints

- Rust **edition 2024**, `rust-version = 1.96`.
- **No `unwrap`/`expect` outside `#[cfg(test)]`.** Fallible code returns `io::Result` + `?`; ad-hoc errors via `io::Error::other(...)`.
- **No new crate dependencies.** File locking uses `std::fs::File::try_lock` + `std::fs::TryLockError` (available in 1.96).
- **Non-destructive injection ONLY** — the controller uses `pane split`, never `layout.apply`. Auto-injection is scoped to **single-pane** tabs (Phase 3 spike: `layout.apply` kills processes; `pane split` is full-width only on single-pane tabs). See GOAL.md and `docs/decisions.md`.
- Branch **`feat/phase-3-controller`** (already checked out, stacked on the Phase 2 tip); never commit to `main`.
- **TDD:** failing test first, watch it fail, minimal implementation, watch it pass, commit.
- Sentence-style test names; `//!` module headers; `///` on public items.
- Scope: Phase 3 only. **No** config knobs (sweep interval, scope, height/motion/palette), packaging, or Kitty sprites — those are Phase 4.

## File Structure

- `src/lock.rs` (create) — single-owner advisory lock (`acquire` + `LockGuard`).
- `src/control.rs` (create) — pure sweep logic (`TabRef`/`PaneRef`, parsing, eligibility, `plan_injections`), `inject_strip`, `sweep_once`, the `control` loop, `controller_lock_path`.
- `src/lib.rs` (modify) — register `pub mod lock;` and `pub mod control;`.
- `src/main.rs` (modify) — dispatch the `control` subcommand; update usage.
- `herdr-plugin.toml` (modify) — add the `start-pets-controller` `[[actions]]` entry.
- `tests/manifest.rs` (modify) — assert the new action.

---

## Task 1: Single-owner lock (`lock.rs`)

**Files:**
- Create: `src/lock.rs`
- Modify: `src/lib.rs` (register the module)

**Interfaces:**
- Produces: `pub struct LockGuard;` and `pub fn acquire(path: &Path) -> io::Result<Option<LockGuard>>` — `Ok(Some(guard))` when the exclusive lock is taken, `Ok(None)` when another process already holds it. The lock releases when `LockGuard` drops (or the process exits).

- [ ] **Step 1: Register the module** — add to `src/lib.rs`, keeping the list alphabetical (between `identity` and `palette`):

```rust
pub mod lock;
```

- [ ] **Step 2: Create `src/lock.rs` with the failing tests first:**

```rust
//! Single-owner advisory lock so only one `control` watchdog runs per session.
//! Uses `std::fs::File::try_lock` (stable since Rust 1.89) — no external crate.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_grants_the_lock_then_blocks_a_second_holder() {
        let path = std::env::temp_dir().join(format!("herdr-pets-lock-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let first = acquire(&path).unwrap();
        assert!(first.is_some(), "first acquire takes the lock");

        let second = acquire(&path).unwrap();
        assert!(second.is_none(), "second acquire is blocked while the first is held");

        drop(first);
        let third = acquire(&path).unwrap();
        assert!(third.is_some(), "dropping the guard frees the lock");

        drop(third);
        let _ = std::fs::remove_file(&path);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail (do not compile)**

Run: `cargo test -p herdr-pets --lib lock::`
Expected: FAIL — `cannot find function 'acquire'`.

- [ ] **Step 4: Implement `acquire` + `LockGuard`** — insert above the `#[cfg(test)]` module:

```rust
/// Held for the controller's lifetime. The advisory lock is released when this
/// guard drops (the file is closed) — so a crash frees it too.
pub struct LockGuard {
    _file: File,
}

/// Try to take an exclusive, non-blocking advisory lock on `path`. Returns
/// `Ok(Some(guard))` if acquired, `Ok(None)` if another process holds it (a
/// normal outcome — a second controller should just exit).
pub fn acquire(path: &Path) -> io::Result<Option<LockGuard>> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(LockGuard { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p herdr-pets --lib lock::`
Expected: PASS.

- [ ] **Step 6: Format, lint, commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy -p herdr-pets --all-targets -- -D warnings`
Expected: clean.

```bash
git add src/lock.rs src/lib.rs
git commit -m "feat(lock): single-owner advisory lock for the controller"
```

---

## Task 2: Controller pure logic (`control.rs`)

**Files:**
- Create: `src/control.rs` (types, parsing, eligibility, plan — plus its tests)
- Modify: `src/lib.rs` (register the module)

**Interfaces:**
- Produces:
  - `pub struct TabRef { pub tab_id: String, pub pane_count: u32 }`
  - `pub struct PaneRef { pub pane_id: String, pub tab_id: String, pub label: Option<String> }`
  - `pub fn parse_tabs(list_json: &str) -> io::Result<Vec<TabRef>>`
  - `pub fn parse_panes(list_json: &str) -> io::Result<Vec<PaneRef>>`
  - `pub const STRIP_LABEL: &str` and `pub fn is_strip_label(label: &str) -> bool`
  - `pub fn tabs_with_strip(panes: &[PaneRef]) -> HashSet<String>`
  - `pub fn needs_strip(tab: &TabRef, with_strip: &HashSet<String>) -> bool`
  - `pub fn plan_injections(tabs: &[TabRef], panes: &[PaneRef]) -> Vec<(String, String)>` — `(tab_id, root_pane_id)` pairs to inject.

- [ ] **Step 1: Register the module** — add to `src/lib.rs`, alphabetically (between `control`… note it sorts before `herd`): add the line

```rust
pub mod control;
```

(Place it right after `pub mod anim;` so the list stays alphabetical: `agent, anim, control, herd, …`.)

- [ ] **Step 2: Create `src/control.rs` with the module header, uses, and failing tests first:**

```rust
//! The controller/watchdog: keep a slim full-width pet strip in every eligible
//! tab via a non-destructive `pane split` (never `layout.apply`, which kills
//! processes — Phase 3 spike). Pure sweep logic here; the loop + I/O are thin
//! shells over the `herdr` CLI seam. See the Phase 3 design spec.

use std::collections::HashSet;
use std::io;

use serde_json::Value;

// NOTE: Tasks 3 and 4 add the imports their code needs (`crate::herdr::HerdrCli`,
// `crate::place::{...}`, `std::path::{Path, PathBuf}`, `std::time::Duration`,
// `crate::{lock, socket}`). Task 2 imports ONLY what its pure logic uses, so this
// commit stays clippy-clean under `-D warnings`.

/// The pane label the controller stamps on each strip so later sweeps (and a
/// fresh controller after a restart) recognise it and never stack a second one.
pub const STRIP_LABEL: &str = "herdr-pets";

/// A tab as seen by `herdr tab list`.
#[derive(Debug, Clone, PartialEq)]
pub struct TabRef {
    pub tab_id: String,
    pub pane_count: u32,
}

/// A pane as seen by `herdr pane list` (label is set only on some panes).
#[derive(Debug, Clone, PartialEq)]
pub struct PaneRef {
    pub pane_id: String,
    pub tab_id: String,
    pub label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: &str, panes: u32) -> TabRef {
        TabRef { tab_id: id.into(), pane_count: panes }
    }
    fn pane(id: &str, tab: &str, label: Option<&str>) -> PaneRef {
        PaneRef { pane_id: id.into(), tab_id: tab.into(), label: label.map(String::from) }
    }

    #[test]
    fn parse_tabs_reads_id_and_pane_count() {
        let j = r#"{"result":{"tabs":[
            {"tab_id":"w1:t1","pane_count":1,"label":"a"},
            {"tab_id":"w1:t2","pane_count":3,"label":"b"}]}}"#;
        let tabs = parse_tabs(j).unwrap();
        assert_eq!(tabs, vec![tab("w1:t1", 1), tab("w1:t2", 3)]);
    }

    #[test]
    fn parse_panes_reads_optional_label() {
        let j = r#"{"result":{"panes":[
            {"pane_id":"w1:p1","tab_id":"w1:t1"},
            {"pane_id":"w1:p2","tab_id":"w1:t1","label":"herdr-pets"}]}}"#;
        let panes = parse_panes(j).unwrap();
        assert_eq!(panes[0], pane("w1:p1", "w1:t1", None));
        assert_eq!(panes[1], pane("w1:p2", "w1:t1", Some("herdr-pets")));
    }

    #[test]
    fn is_strip_label_matches_the_marker_and_the_manifest_title() {
        assert!(is_strip_label("herdr-pets"));
        assert!(is_strip_label("Pets"));
        assert!(!is_strip_label("claude"));
    }

    #[test]
    fn tabs_with_strip_collects_tabs_that_hold_a_marked_pane() {
        let panes = vec![
            pane("w1:p1", "w1:t1", None),
            pane("w1:p2", "w1:t1", Some("herdr-pets")),
            pane("w1:p3", "w1:t2", Some("claude")),
        ];
        let with = tabs_with_strip(&panes);
        assert!(with.contains("w1:t1"));
        assert!(!with.contains("w1:t2"), "an agent pane is not a strip");
    }

    #[test]
    fn needs_strip_is_true_only_for_a_single_pane_tab_without_one() {
        let mut with = HashSet::new();
        assert!(needs_strip(&tab("w1:t1", 1), &with), "single-pane, no strip");
        assert!(!needs_strip(&tab("w1:t2", 2), &with), "multi-pane is skipped");
        with.insert("w1:t1".to_string());
        assert!(!needs_strip(&tab("w1:t1", 1), &with), "already has a strip");
    }

    #[test]
    fn plan_injections_pairs_eligible_tabs_with_their_sole_pane() {
        let tabs = vec![tab("w1:t1", 1), tab("w1:t2", 2), tab("w1:t3", 1)];
        let panes = vec![
            pane("w1:p1", "w1:t1", None),                 // eligible
            pane("w1:p2", "w1:t2", None),                 // multi-pane, skipped
            pane("w1:p9", "w1:t2", None),
            pane("w1:pA", "w1:t3", Some("Pets")),         // already stripped
        ];
        let plan = plan_injections(&tabs, &panes);
        assert_eq!(plan, vec![("w1:t1".to_string(), "w1:p1".to_string())]);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail (do not compile)**

Run: `cargo test -p herdr-pets --lib control::`
Expected: FAIL — `cannot find function 'parse_tabs'` (and the others).

- [ ] **Step 4: Implement the pure helpers** — insert above the `#[cfg(test)]` module:

```rust
/// `true` if a pane label marks it as a pets strip — the controller's own
/// marker or the manifest pane title (so a `place`/manual strip is deduped too).
pub fn is_strip_label(label: &str) -> bool {
    label == STRIP_LABEL || label == "Pets"
}

/// Parse `herdr tab list` into `TabRef`s (skips malformed entries tolerantly).
pub fn parse_tabs(list_json: &str) -> io::Result<Vec<TabRef>> {
    let v: Value = serde_json::from_str(list_json).map_err(io::Error::other)?;
    let tabs = v
        .get("result")
        .and_then(|r| r.get("tabs"))
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("no result.tabs in tab list output"))?;
    Ok(tabs
        .iter()
        .filter_map(|t| {
            Some(TabRef {
                tab_id: t.get("tab_id")?.as_str()?.to_string(),
                pane_count: t.get("pane_count").and_then(Value::as_u64).unwrap_or(0) as u32,
            })
        })
        .collect())
}

/// Parse `herdr pane list` into `PaneRef`s (`label` present only when set).
pub fn parse_panes(list_json: &str) -> io::Result<Vec<PaneRef>> {
    let v: Value = serde_json::from_str(list_json).map_err(io::Error::other)?;
    let panes = v
        .get("result")
        .and_then(|r| r.get("panes"))
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("no result.panes in pane list output"))?;
    Ok(panes
        .iter()
        .filter_map(|p| {
            Some(PaneRef {
                pane_id: p.get("pane_id")?.as_str()?.to_string(),
                tab_id: p.get("tab_id")?.as_str()?.to_string(),
                label: p.get("label").and_then(Value::as_str).map(String::from),
            })
        })
        .collect())
}

/// The set of tab ids that already contain a strip pane (by label marker).
pub fn tabs_with_strip(panes: &[PaneRef]) -> HashSet<String> {
    panes
        .iter()
        .filter(|p| p.label.as_deref().is_some_and(is_strip_label))
        .map(|p| p.tab_id.clone())
        .collect()
}

/// A tab is eligible for non-destructive full-width injection iff it has exactly
/// one pane and no strip yet. (Multi-pane tabs can't get a full-width strip
/// without the destructive `layout.apply`, so they are left to on-demand `place`.)
pub fn needs_strip(tab: &TabRef, with_strip: &HashSet<String>) -> bool {
    tab.pane_count == 1 && !with_strip.contains(&tab.tab_id)
}

/// The `(tab_id, root_pane_id)` pairs to inject this sweep: each eligible tab
/// paired with its sole pane (the split target).
pub fn plan_injections(tabs: &[TabRef], panes: &[PaneRef]) -> Vec<(String, String)> {
    let with_strip = tabs_with_strip(panes);
    tabs.iter()
        .filter(|t| needs_strip(t, &with_strip))
        .filter_map(|t| {
            let pane = panes.iter().find(|p| p.tab_id == t.tab_id)?;
            Some((t.tab_id.clone(), pane.pane_id.clone()))
        })
        .collect()
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p herdr-pets --lib control::`
Expected: PASS (all six).

- [ ] **Step 6: Format, lint, commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy -p herdr-pets --all-targets -- -D warnings`
Expected: clean. (Task 2 imports only `std::collections::HashSet`, `std::io`, `serde_json::Value` — exactly what its pure logic uses — so there are no unused-import warnings.)

```bash
git add src/control.rs src/lib.rs
git commit -m "feat(control): pure tab/pane parsing and strip-eligibility logic"
```

---

## Task 3: Non-destructive strip injection (`inject_strip`)

**Files:**
- Modify: `src/control.rs` (add `inject_strip` + `parse_split_pane_id` + test)

**Interfaces:**
- Consumes: `HerdrCli` (`crate::herdr`), `place::{slim_ratio, TARGET_ROWS, parse_tab_rows}`, `STRIP_LABEL`.
- Produces: `pub fn inject_strip(cli: &dyn HerdrCli, root_pane_id: &str, self_exe: &str) -> io::Result<()>`.

- [ ] **Step 1: Add imports** — at the top of `src/control.rs`, add (if not already present from Task 2):

```rust
use crate::herdr::HerdrCli;
use crate::place::{TARGET_ROWS, parse_tab_rows, slim_ratio};
```

- [ ] **Step 2: Write the failing test** — add to the `tests` module in `src/control.rs`:

```rust
    use std::cell::RefCell;

    struct FakeCli {
        calls: RefCell<Vec<Vec<String>>>,
    }
    impl FakeCli {
        fn new() -> Self {
            FakeCli { calls: RefCell::new(Vec::new()) }
        }
    }
    impl HerdrCli for FakeCli {
        fn run_json(&self, args: &[&str]) -> io::Result<String> {
            self.calls.borrow_mut().push(args.iter().map(|s| s.to_string()).collect());
            match args {
                ["tab", "list"] => Ok(r#"{"result":{"tabs":[{"tab_id":"w1:t1","pane_count":1}]}}"#.into()),
                ["pane", "list"] => Ok(r#"{"result":{"panes":[{"pane_id":"w1:p1","tab_id":"w1:t1"}]}}"#.into()),
                ["pane", "layout", ..] => Ok(r#"{"result":{"layout":{"area":{"height":64}}}}"#.into()),
                ["pane", "split", ..] => Ok(r#"{"result":{"pane":{"pane_id":"w1:pNEW"}}}"#.into()),
                _ => Ok(r#"{"result":{}}"#.into()),
            }
        }
    }

    #[test]
    fn inject_strip_splits_runs_and_labels_in_order() {
        let cli = FakeCli::new();
        inject_strip(&cli, "w1:p1", "/abs/herdr-pets").unwrap();
        let calls = cli.calls.borrow();
        assert_eq!(calls[0], vec!["pane", "layout", "--pane", "w1:p1"]);
        // slim_ratio(64, 7) = 1 - 7/64 = 0.890625 -> "{:.4}" = "0.8906"
        assert_eq!(
            calls[1],
            vec!["pane", "split", "w1:p1", "--direction", "down", "--ratio", "0.8906", "--no-focus"]
        );
        assert_eq!(calls[2], vec!["pane", "run", "w1:pNEW", "/abs/herdr-pets render"]);
        assert_eq!(calls[3], vec!["pane", "rename", "w1:pNEW", "herdr-pets"]);
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p herdr-pets --lib control::tests::inject_strip_splits_runs_and_labels_in_order`
Expected: FAIL — `cannot find function 'inject_strip'`.

- [ ] **Step 4: Implement `inject_strip` + `parse_split_pane_id`** — add above the `#[cfg(test)]` module:

```rust
/// Non-destructively place a slim full-width pets strip below `root_pane_id`
/// (the sole pane of a single-pane tab): measure the tab, split down at the slim
/// ratio, run the renderer in the new pane, and stamp the de-dup label. Uses
/// `pane split` (NOT `layout.apply`) so the existing pane's process survives.
pub fn inject_strip(cli: &dyn HerdrCli, root_pane_id: &str, self_exe: &str) -> io::Result<()> {
    let layout_json = cli.run_json(&["pane", "layout", "--pane", root_pane_id])?;
    let rows = parse_tab_rows(&layout_json)?;
    let ratio_arg = format!("{:.4}", slim_ratio(rows, TARGET_ROWS));
    let split_reply = cli.run_json(&[
        "pane", "split", root_pane_id, "--direction", "down", "--ratio", &ratio_arg, "--no-focus",
    ])?;
    let strip_pane = parse_split_pane_id(&split_reply)?;
    let render_cmd = format!("{self_exe} render");
    cli.run_json(&["pane", "run", &strip_pane, &render_cmd])?;
    cli.run_json(&["pane", "rename", &strip_pane, STRIP_LABEL])?;
    Ok(())
}

/// Extract `result.pane.pane_id` from a `herdr pane split` reply.
fn parse_split_pane_id(reply: &str) -> io::Result<String> {
    let v: Value = serde_json::from_str(reply).map_err(io::Error::other)?;
    v.get("result")
        .and_then(|r| r.get("pane"))
        .and_then(|p| p.get("pane_id"))
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| io::Error::other("no result.pane.pane_id in pane split reply"))
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p herdr-pets --lib control::tests::inject_strip_splits_runs_and_labels_in_order`
Expected: PASS.

- [ ] **Step 6: Format, lint, commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy -p herdr-pets --all-targets -- -D warnings`
Expected: clean.

```bash
git add src/control.rs
git commit -m "feat(control): non-destructive pane-split strip injection"
```

---

## Task 4: Sweep + control loop + dispatch

**Files:**
- Modify: `src/control.rs` (`sweep_once`, `control`, `controller_lock_path` + a `sweep_once` test)
- Modify: `src/main.rs` (dispatch `control`; update usage)

**Interfaces:**
- Consumes: `plan_injections`, `inject_strip`, `parse_tabs`, `parse_panes`, `lock::acquire`, `socket::socket_path`.
- Produces:
  - `pub fn sweep_once(cli: &dyn HerdrCli, self_exe: &str) -> io::Result<()>`
  - `pub fn control(cli: &dyn HerdrCli, self_exe: &str, lock_path: &Path, interval: Duration) -> io::Result<()>`
  - `pub fn controller_lock_path() -> PathBuf`

- [ ] **Step 1: Add imports** — at the top of `src/control.rs`, ensure these are present:

```rust
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{lock, socket};
```

- [ ] **Step 2: Write the failing `sweep_once` test** — add to the `tests` module. It extends `FakeCli` from Task 3 to record whether the eligible tab (and only it) was injected:

```rust
    struct SweepFake {
        calls: RefCell<Vec<Vec<String>>>,
        tabs: String,
        panes: String,
    }
    impl HerdrCli for SweepFake {
        fn run_json(&self, args: &[&str]) -> io::Result<String> {
            self.calls.borrow_mut().push(args.iter().map(|s| s.to_string()).collect());
            match args {
                ["tab", "list"] => Ok(self.tabs.clone()),
                ["pane", "list"] => Ok(self.panes.clone()),
                ["pane", "layout", ..] => Ok(r#"{"result":{"layout":{"area":{"height":64}}}}"#.into()),
                ["pane", "split", ..] => Ok(r#"{"result":{"pane":{"pane_id":"w1:pNEW"}}}"#.into()),
                _ => Ok(r#"{"result":{}}"#.into()),
            }
        }
    }

    #[test]
    fn sweep_once_injects_only_the_eligible_tab() {
        // t1: single-pane, no strip -> inject. t2: multi-pane -> skip.
        // t3: single-pane but already stripped -> skip.
        let cli = SweepFake {
            calls: RefCell::new(Vec::new()),
            tabs: r#"{"result":{"tabs":[
                {"tab_id":"w1:t1","pane_count":1},
                {"tab_id":"w1:t2","pane_count":2},
                {"tab_id":"w1:t3","pane_count":1}]}}"#.into(),
            panes: r#"{"result":{"panes":[
                {"pane_id":"w1:p1","tab_id":"w1:t1"},
                {"pane_id":"w1:p2","tab_id":"w1:t2"},
                {"pane_id":"w1:p9","tab_id":"w1:t2"},
                {"pane_id":"w1:pA","tab_id":"w1:t3","label":"herdr-pets"}]}}"#.into(),
        };
        sweep_once(&cli, "/abs/herdr-pets").unwrap();
        let calls = cli.calls.borrow();
        // Exactly one split, targeting t1's sole pane w1:p1.
        let splits: Vec<&Vec<String>> = calls.iter().filter(|c| c.first().map(String::as_str) == Some("pane") && c.get(1).map(String::as_str) == Some("split")).collect();
        assert_eq!(splits.len(), 1, "only the one eligible tab is injected");
        assert_eq!(splits[0][2], "w1:p1");
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p herdr-pets --lib control::tests::sweep_once_injects_only_the_eligible_tab`
Expected: FAIL — `cannot find function 'sweep_once'`.

- [ ] **Step 4: Implement `sweep_once`, `control`, `controller_lock_path`** — add above the `#[cfg(test)]` module:

```rust
/// One sweep: list tabs + panes, then inject a strip into every eligible tab.
/// A per-tab injection failure is logged and skipped so one bad tab never
/// aborts the sweep or the other tabs (unobtrusive).
pub fn sweep_once(cli: &dyn HerdrCli, self_exe: &str) -> io::Result<()> {
    let tabs = parse_tabs(&cli.run_json(&["tab", "list"])?)?;
    let panes = parse_panes(&cli.run_json(&["pane", "list"])?)?;
    for (tab_id, root_pane) in plan_injections(&tabs, &panes) {
        if let Err(e) = inject_strip(cli, &root_pane, self_exe) {
            eprintln!("herdr-pets: could not place strip in {tab_id}: {e}");
        }
    }
    Ok(())
}

/// Run the watchdog: take the single-owner lock (exit cleanly if another
/// controller holds it), then sweep every `interval` forever. The poll unifies
/// startup, new-tab injection, and respawn/re-assert (a closed strip reappears
/// next sweep). A failed whole sweep is logged and retried next interval.
pub fn control(
    cli: &dyn HerdrCli,
    self_exe: &str,
    lock_path: &Path,
    interval: Duration,
) -> io::Result<()> {
    let _guard = match lock::acquire(lock_path)? {
        Some(g) => g,
        None => {
            eprintln!("herdr-pets: another controller is already running; exiting");
            return Ok(());
        }
    };
    loop {
        if let Err(e) = sweep_once(cli, self_exe) {
            eprintln!("herdr-pets: sweep failed: {e}");
        }
        std::thread::sleep(interval);
    }
}

/// Session-scoped path for the controller lock: next to the herdr socket if
/// known, else the system temp dir.
pub fn controller_lock_path() -> PathBuf {
    socket::socket_path()
        .and_then(|p| p.parent().map(|d| d.join("herdr-pets-controller.lock")))
        .unwrap_or_else(|| std::env::temp_dir().join("herdr-pets-controller.lock"))
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p herdr-pets --lib control::tests::sweep_once_injects_only_the_eligible_tab`
Expected: PASS.

- [ ] **Step 6: Dispatch `control` from `main.rs`** — add the arm (after the `place` arm) in `src/main.rs`:

```rust
        Some("control") => {
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
                std::time::Duration::from_millis(3000),
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("herdr-pets: {e}");
                    ExitCode::FAILURE
                }
            }
        }
```

Update the usage fallthrough:

```rust
        _ => {
            eprintln!("usage: herdr-pets render|place|control");
            ExitCode::FAILURE
        }
```

- [ ] **Step 7: Build and run the full suite**

Run: `cargo build -p herdr-pets && cargo test -p herdr-pets`
Expected: PASS — compiles, all tests pass. (`control`'s infinite loop is exercised live in Task 6, not by a unit test.)

- [ ] **Step 8: Format, lint, commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy -p herdr-pets --all-targets -- -D warnings`
Expected: clean.

```bash
git add src/control.rs src/main.rs
git commit -m "feat(control): poll sweep, watchdog loop, and control subcommand"
```

---

## Task 5: Manifest action + test

**Files:**
- Modify: `herdr-plugin.toml` (add the `[[actions]]` entry)
- Modify: `tests/manifest.rs` (assert it)

**Interfaces:**
- Produces: a second `[[actions]]` entry `start-pets-controller` → `["./target/release/herdr-pets", "control"]`.

- [ ] **Step 1: Write the failing test** — add to `tests/manifest.rs`:

```rust
#[test]
fn manifest_action_starts_the_controller_via_the_release_binary() {
    let m = manifest();
    let actions = m.get("actions").and_then(Value::as_array).expect("[[actions]] present");
    let ctrl = actions
        .iter()
        .find(|a| a.get("id").and_then(Value::as_str) == Some("start-pets-controller"))
        .expect("start-pets-controller action present");
    let cmd: Vec<&str> = ctrl
        .get("command")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(cmd, vec!["./target/release/herdr-pets", "control"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p herdr-pets --test manifest manifest_action_starts_the_controller_via_the_release_binary`
Expected: FAIL — panic `start-pets-controller action present`.

- [ ] **Step 3: Add the `[[actions]]` entry** — append to `herdr-plugin.toml`:

```toml

[[actions]]
id = "start-pets-controller"
title = "Start pets controller"
command = ["./target/release/herdr-pets", "control"]
```

- [ ] **Step 4: Run the manifest tests to verify they pass**

Run: `cargo test -p herdr-pets --test manifest`
Expected: PASS (existing manifest tests plus the new one).

- [ ] **Step 5: Format, lint, commit**

Run: `cargo fmt --check && cargo clippy -p herdr-pets --all-targets -- -D warnings`
Expected: clean.

```bash
git add herdr-plugin.toml tests/manifest.rs
git commit -m "feat(control): add the start-pets-controller manifest action"
```

---

## Task 6: Live verification & wrap-up

No new unit tests — verify the things unit tests cannot (non-destructive injection in a real session, respawn, single-owner lock, multi-pane skip), then mark the tracker. **Use an isolated scratch workspace/tab** so a mishap stays away from live work.

- [ ] **Step 1: Build the release binary**

Run: `cargo build --release -p herdr-pets`
Expected: builds `./target/release/herdr-pets`.

- [ ] **Step 2: Re-link the plugin**

Run: `herdr plugin link .` then `herdr plugin action list --plugin herdr-pets` — expect both `place-pets` and `start-pets-controller`.

- [ ] **Step 3: Verify non-destructive auto-injection + new-tab + respawn**

- Start the controller (backgrounded or in a scratch pane): `./target/release/herdr-pets control &` (note its PID; `kill` it at the end).
- Create a scratch **single-pane** tab and start a marker process in it (`herdr pane run <pane> "sleep 99999 # MARK"`); note the marker PID.
- Within ~3s, confirm a slim full-width strip appears at the bottom of that tab (`herdr pane layout --pane <pane>`: a full-width `height≈7` bottom pane) running the renderer, and the marker PID is **still alive** (`ps -p <pid>`).
- Close the strip pane (`herdr pane close <strip>`); within ~3s confirm it **returns** (respawn) and the marker is still alive.

Expected: strip appears and returns; the marker process is never killed. **If injection kills the marker**, stop and flag it (update GOAL.md/PLAN.md/decisions.md) — it contradicts the Phase 3 spike.

- [ ] **Step 4: Verify single-owner lock**

Run a second `./target/release/herdr-pets control` in the foreground.
Expected: it prints `another controller is already running; exiting` and exits 0 without injecting.

- [ ] **Step 5: Verify multi-pane tabs are skipped**

Split the scratch tab so it has 2+ panes with no strip, or observe an existing multi-pane tab.
Expected: the controller does **not** inject there (no rebuild, no killed processes).

- [ ] **Step 6: Clean up**

`kill` the controller PID; close scratch tabs.

- [ ] **Step 7: Run the full gate**

Run: `cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check`
Expected: all pass, no warnings.

- [ ] **Step 8: Update the phase tracker** — in `docs/PLAN.md`, set the Phase 3 row to:

```markdown
| 3 | Always everywhere (controller) | Done | [design](superpowers/specs/2026-07-23-phase-3-controller-design.md) | [plan](superpowers/plans/2026-07-23-phase-3-controller.md) |
```

- [ ] **Step 9: Commit**

```bash
git add docs/PLAN.md
git commit -m "docs(phase-3): mark Phase 3 done and link the plan"
```

---

## Self-Review

**Spec coverage:**
- §2 non-destructive constraint → Tasks 3 (`inject_strip` uses `pane split`, never `layout.apply`), 6 (live-verified marker survives).
- §4.1 pure logic (`TabRef`/`PaneRef`/parse/`is_strip_label`/`tabs_with_strip`/`needs_strip`/`plan_injections`) → Task 2.
- §4.2 `inject_strip` (layout→ratio→split→run→rename) → Task 3.
- §4.3 `sweep_once` + `control` poll loop → Task 4.
- §4.4 single-owner `flock` → Task 1.
- §5 data flow (control → lock → loop → inject) → Task 4.
- §7 testing (parse, eligibility, plan, inject sequence, lock contention, sweep) → Tasks 1–4; live items → Task 6.
- §8 manifest action → Task 5.

**Placeholder scan:** No TBD/TODO. Every code step shows complete code; every command shows expected output. (Task 2 Step 6 notes trimming imports to only those used at that commit — the full import set lands in Task 3/4 as those functions are added, keeping each commit clippy-clean.)

**Type consistency:** `inject_strip(&dyn HerdrCli, &str, &str) -> io::Result<()>`, `sweep_once(&dyn HerdrCli, &str)`, `control(&dyn HerdrCli, &str, &Path, Duration)`, `plan_injections(&[TabRef], &[PaneRef]) -> Vec<(String,String)>`, `acquire(&Path) -> io::Result<Option<LockGuard>>` are used identically across tasks. `STRIP_LABEL = "herdr-pets"` matches the `pane rename` arg and the `inject_strip` test assertion. `slim_ratio(64,7)=0.890625` → `"{:.4}"` → `"0.8906"` matches the Task 3 test.
