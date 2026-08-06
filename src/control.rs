//! The controller/watchdog: keep a slim full-width herd strip in every eligible
//! tab via a non-destructive `pane split` (never `layout.apply`, which kills
//! processes — Phase 3 spike). Pure sweep logic here; the loop + I/O are thin
//! shells over the `herdr` CLI seam. See the Phase 3 design spec.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::herdr::HerdrCli;
use crate::place::slim_ratio;
use crate::{lock, socket};

/// The pane label the controller stamps on each strip so later sweeps (and a
/// fresh controller after a restart) recognise it and never stack a second one.
pub const STRIP_LABEL: &str = "herdr-herd";

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

/// `true` if a pane label marks it as a herd strip — the controller's own
/// marker or the manifest pane title (so a `place`/manual strip is deduped too).
pub fn is_strip_label(label: &str) -> bool {
    label == STRIP_LABEL || label == "Herd"
}

/// The pane to split for a full-width strip, plus its row count. The split
/// ratio is computed relative to this pane (not the whole tab), so the strip is
/// the target height wherever the pane sits in the layout.
#[derive(Debug, Clone, PartialEq)]
pub struct StripTarget {
    pub pane_id: String,
    pub pane_rows: u16,
}

/// From a `herdr pane layout` reply, find the pane to split for a **full-width
/// bottom** strip: a leaf pane whose `rect` spans the tab width
/// (`x == area.x && width == area.width`) and sits on the tab's bottom edge
/// (`y + height == area.y + area.height`). Splitting it `down` is
/// non-destructive and yields a full-width strip.
///
/// - A single-pane tab's sole pane qualifies (unchanged coverage).
/// - A "content on top, full-width pane across the bottom" multi-pane tab
///   qualifies (new coverage) — the bottom-most such pane is chosen.
/// - A tab whose bottom edge is split into side-by-side columns has no
///   full-width bottom pane ⇒ `None` (can't get a full-width strip without the
///   process-killing `layout.apply`; left to the on-demand `place`).
///
/// Tolerant: malformed or absent geometry ⇒ `None` (the tab is skipped).
pub fn find_bottom_strip_target(layout_json: &str) -> Option<StripTarget> {
    let v: Value = serde_json::from_str(layout_json).ok()?;
    let layout = v.get("result")?.get("layout")?;
    let area = layout.get("area")?;
    let ax = area.get("x")?.as_i64()?;
    let ay = area.get("y")?.as_i64()?;
    let aw = area.get("width")?.as_i64()?;
    let ah = area.get("height")?.as_i64()?;
    let tab_bottom = ay + ah;

    let mut best: Option<StripTarget> = None;
    let mut best_y = i64::MIN;
    for p in layout.get("panes")?.as_array()? {
        let Some(id) = p.get("pane_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(r) = p.get("rect") else { continue };
        let get = |k: &str| r.get(k).and_then(Value::as_i64);
        let (Some(rx), Some(ry), Some(rw), Some(rh)) =
            (get("x"), get("y"), get("width"), get("height"))
        else {
            continue;
        };
        let full_width = rx == ax && rw == aw;
        let bottom_aligned = ry + rh == tab_bottom;
        // Bottom-most full-width pane wins (there should be exactly one).
        if full_width && bottom_aligned && ry > best_y {
            best_y = ry;
            best = Some(StripTarget {
                pane_id: id.to_string(),
                pane_rows: rh.max(0) as u16,
            });
        }
    }
    best
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

/// A tab is a candidate for injection iff it does not already hold a strip.
/// Whether a full-width strip can actually be placed non-destructively is
/// decided per-tab from its layout geometry ([`find_bottom_strip_target`]) —
/// single-pane tabs and multi-pane tabs with a full-width bottom pane qualify;
/// a columned-bottom tab yields no target and is skipped.
pub fn needs_strip(tab: &TabRef, with_strip: &HashSet<String>) -> bool {
    !with_strip.contains(&tab.tab_id)
}

/// The `(tab_id, probe_pane_id)` pairs to consider this sweep: every tab without
/// a strip, paired with one of its pane ids (used only to fetch that tab's
/// layout; `find_bottom_strip_target` then picks the actual split target).
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

/// The strips the *controller* injected — the subset a reload may restart.
/// Narrower than [`strip_panes`] on purpose: the sweep can only put back what
/// it created, so closing a manifest-opened `Herd` pane could lose a strip for
/// good in a tab the sweep cannot inject into.
pub fn controller_strips(panes: &[PaneRef]) -> Vec<String> {
    panes
        .iter()
        .filter(|p| p.label.as_deref() == Some(STRIP_LABEL))
        .map(|p| p.pane_id.clone())
        .collect()
}

/// The strip panes to close so each tab is left holding exactly one: every
/// strip after the first in each tab. Injection alone cannot guarantee this —
/// it only ever *adds* — so the sweep reaps whatever a lost label, a restored
/// session, or a `place` racing the sweep left behind.
pub fn plan_reap(panes: &[PaneRef]) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    panes
        .iter()
        .filter(|p| p.label.as_deref().is_some_and(is_strip_label))
        .filter(|p| !seen.insert(p.tab_id.as_str()))
        .map(|p| p.pane_id.clone())
        .collect()
}

/// Close every strip pane. Best-effort and unordered: a pane that will not
/// close is logged and skipped, because the alternative — aborting — would
/// leave the herd in a half-restarted state.
pub fn close_strips(cli: &dyn HerdrCli, panes: &[PaneRef]) {
    for pane_id in controller_strips(panes) {
        if let Err(e) = cli.run_json(&["pane", "close", &pane_id]) {
            eprintln!("herdr-herd: could not close strip {pane_id}: {e}");
        }
    }
}

/// `true` when the binary on disk is not the one this process started from.
/// Both stamps must be readable: a transient stat failure must never look like
/// a change, or a single unreadable moment would restart every strip on a loop.
pub fn binary_changed(baseline: Option<u64>, current: Option<u64>) -> bool {
    match (baseline, current) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    }
}

/// The binary's modification time as opaque nanoseconds, or `None` if it cannot
/// be read. Only ever compared for equality — the value itself means nothing.
pub fn binary_stamp(path: &str) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(since.as_nanos() as u64)
}

/// Non-destructively place a slim full-width herd strip below `target_pane`
/// (a full-width bottom pane found by [`find_bottom_strip_target`]): split it
/// `down` at the slim ratio, run the renderer in the new pane, and stamp the
/// de-dup label. The ratio is relative to `pane_rows` (the target pane's own
/// height), so the strip is ~`target_rows` tall wherever the pane sits. Uses
/// `pane split` (NOT `layout.apply`), so the split pane's process survives.
pub fn inject_strip(
    cli: &dyn HerdrCli,
    target_pane: &str,
    pane_rows: u16,
    self_exe: &str,
    target_rows: u16,
) -> io::Result<()> {
    let ratio_arg = format!("{:.4}", slim_ratio(pane_rows, target_rows));
    let split_reply = cli.run_json(&[
        "pane",
        "split",
        target_pane,
        "--direction",
        "down",
        "--ratio",
        &ratio_arg,
        "--no-focus",
    ])?;
    let strip_pane = parse_split_pane_id(&split_reply)?;
    let render_cmd = format!("'{self_exe}' render");
    cli.run_json(&["pane", "run", &strip_pane, &render_cmd])?;
    // An unlabelled strip is invisible to every later sweep, which would then
    // inject a second one into the same tab. Rather than leave that orphan
    // behind, close it and report the injection as failed — the next sweep
    // retries from a clean tab.
    if let Err(e) = cli.run_json(&["pane", "rename", &strip_pane, STRIP_LABEL]) {
        let _ = cli.run_json(&["pane", "close", &strip_pane]);
        return Err(e);
    }
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

/// One sweep: list tabs + panes, then inject a strip into every tab that has a
/// full-width bottom pane (single-pane tabs and top+bottom multi-pane tabs).
/// For each candidate the tab's layout is fetched and
/// [`find_bottom_strip_target`] picks the split target; a tab with no full-width
/// bottom pane (columned bottom) is skipped (left to on-demand `place`). A
/// per-tab failure is logged and skipped so one bad tab never aborts the sweep
/// or the others (unobtrusive).
pub fn sweep_once(cli: &dyn HerdrCli, self_exe: &str, target_rows: u16) -> io::Result<()> {
    let tabs = parse_tabs(&cli.run_json(&["tab", "list"])?)?;
    let panes = parse_panes(&cli.run_json(&["pane", "list"])?)?;
    // Reap before injecting: collapsing a tab to one strip must not be undone
    // by this same sweep deciding the tab still needs one.
    for extra in plan_reap(&panes) {
        if let Err(e) = cli.run_json(&["pane", "close", &extra]) {
            eprintln!("herdr-herd: could not close duplicate strip {extra}: {e}");
        }
    }
    for (tab_id, probe_pane) in plan_injections(&tabs, &panes) {
        let result = (|| -> io::Result<()> {
            let layout = cli.run_json(&["pane", "layout", "--pane", &probe_pane])?;
            match find_bottom_strip_target(&layout) {
                Some(target) => inject_strip(
                    cli,
                    &target.pane_id,
                    target.pane_rows,
                    self_exe,
                    target_rows,
                ),
                None => Ok(()), // columned bottom — no full-width strip possible
            }
        })();
        if let Err(e) = result {
            eprintln!("herdr-herd: could not place strip in {tab_id}: {e}");
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
    target_rows: u16,
) -> io::Result<()> {
    let _guard = match lock::acquire(lock_path)? {
        Some(g) => g,
        None => {
            eprintln!("herdr-herd: another controller is already running; exiting");
            return Ok(());
        }
    };
    let mut baseline = binary_stamp(self_exe);
    loop {
        if binary_changed(baseline, binary_stamp(self_exe)) {
            let err = reload(cli, self_exe);
            // Only reached if the re-exec failed. Adopt the new stamp anyway,
            // so a binary we cannot exec does not restart the herd on every
            // sweep forever, then keep sweeping: this image is stale, but the
            // sweep re-injects the strips just closed, and a stale herd beats
            // no herd.
            eprintln!("herdr-herd: could not re-exec {self_exe}: {err}; staying on this build");
            baseline = binary_stamp(self_exe);
        }
        if let Err(e) = sweep_once(cli, self_exe, target_rows) {
            eprintln!("herdr-herd: sweep failed: {e}");
        }
        std::thread::sleep(interval);
    }
}

/// The binary changed under us: close every strip so none keeps running the old
/// image, then re-exec so the controller is new too. The fresh process sweeps
/// immediately and re-injects the strips from the new binary.
///
/// Only returns if the re-exec failed, in which case this process still holds
/// the controller lock. The lock is *not* dropped first, deliberately: Rust
/// opens files `O_CLOEXEC`, so a successful `exec` releases it at exactly the
/// right moment — after the point of no return — and the successor can take it.
fn reload(cli: &dyn HerdrCli, self_exe: &str) -> io::Error {
    eprintln!("herdr-herd: binary changed; restarting the herd");
    match cli.run_json(&["pane", "list"]).map(|s| parse_panes(&s)) {
        Ok(Ok(panes)) => close_strips(cli, &panes),
        Ok(Err(e)) | Err(e) => {
            eprintln!("herdr-herd: could not list panes to restart them: {e}");
        }
    }
    exec_self(self_exe)
}

/// Replace this process with a fresh `self_exe`, preserving argv. Only returns
/// on failure — a successful `exec` never comes back.
#[cfg(unix)]
fn exec_self(self_exe: &str) -> io::Error {
    use std::os::unix::process::CommandExt;
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::Command::new(self_exe).args(args).exec()
}

/// Session-scoped path for the controller lock: next to the herdr socket if
/// known, else the system temp dir. The lock filename embeds a hash of the
/// full socket path so two herdr sessions that happen to share a socket
/// parent directory don't collide on one lock file — each session's
/// controller gets its own lock, scoped to its specific session socket.
pub fn controller_lock_path() -> PathBuf {
    socket::socket_path()
        .and_then(|p| {
            let parent = p.parent()?.to_path_buf();
            let mut hasher = DefaultHasher::new();
            p.to_string_lossy().hash(&mut hasher);
            let file_name = format!("herdr-herd-controller-{:x}.lock", hasher.finish());
            Some(parent.join(file_name))
        })
        .unwrap_or_else(|| std::env::temp_dir().join("herdr-herd-controller.lock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A `pane layout` reply for a tab whose bottom edge is the full-width pane
    /// `id` (single-pane tab, or a top+bottom multi-pane tab). 64-row tab.
    fn full_width_bottom_layout(id: impl std::fmt::Display) -> String {
        format!(
            r#"{{"result":{{"layout":{{"area":{{"x":0,"y":0,"width":100,"height":64}},"panes":[{{"pane_id":"top","rect":{{"x":0,"y":0,"width":100,"height":57}}}},{{"pane_id":"{id}","rect":{{"x":0,"y":57,"width":100,"height":7}}}}]}}}}}}"#
        )
    }

    /// A `pane layout` reply for a tab whose bottom edge is split into two
    /// side-by-side columns — no full-width bottom pane, so no strip is possible.
    fn columned_bottom_layout() -> String {
        r#"{"result":{"layout":{"area":{"x":0,"y":0,"width":100,"height":64},"panes":[
            {"pane_id":"left","rect":{"x":0,"y":0,"width":50,"height":64}},
            {"pane_id":"right","rect":{"x":50,"y":0,"width":50,"height":64}}]}}}"#
            .into()
    }

    struct FakeCli {
        calls: RefCell<Vec<Vec<String>>>,
    }
    impl FakeCli {
        fn new() -> Self {
            FakeCli {
                calls: RefCell::new(Vec::new()),
            }
        }
    }
    impl HerdrCli for FakeCli {
        fn run_json(&self, args: &[&str]) -> io::Result<String> {
            self.calls
                .borrow_mut()
                .push(args.iter().map(|s| s.to_string()).collect());
            match args {
                ["tab", "list"] => {
                    Ok(r#"{"result":{"tabs":[{"tab_id":"w1:t1","pane_count":1}]}}"#.into())
                }
                ["pane", "list"] => {
                    Ok(r#"{"result":{"panes":[{"pane_id":"w1:p1","tab_id":"w1:t1"}]}}"#.into())
                }
                ["pane", "layout", "--pane", id] => Ok(full_width_bottom_layout(id)),
                ["pane", "split", ..] => Ok(r#"{"result":{"pane":{"pane_id":"w1:pNEW"}}}"#.into()),
                _ => Ok(r#"{"result":{}}"#.into()),
            }
        }
    }

    #[test]
    fn inject_strip_splits_runs_and_labels_in_order() {
        let cli = FakeCli::new();
        // pane_rows = 64 (the target pane's own height) -> slim_ratio(64, 7).
        inject_strip(&cli, "w1:p1", 64, "/abs/herdr-herd", 7).unwrap();
        let calls = cli.calls.borrow();
        // No self-fetch of layout: the sweep already resolved the target + rows.
        // slim_ratio(64, 7) = 1 - 7/64 = 0.890625 -> "{:.4}" = "0.8906"
        assert_eq!(
            calls[0],
            vec![
                "pane",
                "split",
                "w1:p1",
                "--direction",
                "down",
                "--ratio",
                "0.8906",
                "--no-focus"
            ]
        );
        assert_eq!(
            calls[1],
            vec!["pane", "run", "w1:pNEW", "'/abs/herdr-herd' render"]
        );
        assert_eq!(calls[2], vec!["pane", "rename", "w1:pNEW", "herdr-herd"]);
    }

    fn tab(id: &str, panes: u32) -> TabRef {
        TabRef {
            tab_id: id.into(),
            pane_count: panes,
        }
    }
    fn pane(id: &str, tab: &str, label: Option<&str>) -> PaneRef {
        PaneRef {
            pane_id: id.into(),
            tab_id: tab.into(),
            label: label.map(String::from),
        }
    }

    #[test]
    fn a_tab_with_one_strip_has_nothing_to_reap() {
        let panes = vec![
            pane("w1:p1", "w1:t1", None),
            pane("w1:p2", "w1:t1", Some("herdr-herd")),
        ];
        assert!(plan_reap(&panes).is_empty());
    }

    /// The invariant: one strip per tab, always. Whatever produced the second
    /// one (a lost label, a restored session, a `place` racing the sweep), the
    /// next sweep collapses it back to one.
    #[test]
    fn a_tab_with_two_strips_reaps_all_but_the_first() {
        let panes = vec![
            pane("w1:p1", "w1:t1", Some("herdr-herd")),
            pane("w1:p2", "w1:t1", Some("Herd")),
            pane("w1:p3", "w1:t1", Some("herdr-herd")),
        ];
        assert_eq!(plan_reap(&panes), vec!["w1:p2".to_string(), "w1:p3".into()]);
    }

    #[test]
    fn reaping_is_per_tab_so_one_strip_each_across_tabs_is_untouched() {
        let panes = vec![
            pane("w1:p1", "w1:t1", Some("herdr-herd")),
            pane("w1:p2", "w1:t2", Some("herdr-herd")),
        ];
        assert!(plan_reap(&panes).is_empty());
    }

    #[test]
    fn sweep_closes_a_duplicate_strip() {
        let cli = SweepFake {
            calls: RefCell::new(Vec::new()),
            tabs: r#"{"result":{"tabs":[{"tab_id":"w1:t1","pane_count":3}]}}"#.into(),
            panes: r#"{"result":{"panes":[
                {"pane_id":"w1:pA","tab_id":"w1:t1","label":"herdr-herd"},
                {"pane_id":"w1:pB","tab_id":"w1:t1","label":"herdr-herd"},
                {"pane_id":"w1:pC","tab_id":"w1:t1"}]}}"#
                .into(),
        };
        sweep_once(&cli, "/abs/herdr-herd", 7).unwrap();
        let calls = cli.calls.borrow();
        let closes: Vec<&str> = calls
            .iter()
            .filter(|c| c.first().map(String::as_str) == Some("pane"))
            .filter(|c| c.get(1).map(String::as_str) == Some("close"))
            .map(|c| c[2].as_str())
            .collect();
        assert_eq!(closes, vec!["w1:pB"], "the duplicate strip is closed");
    }

    /// A split that succeeds but cannot be labelled leaves a pane the next
    /// sweep cannot recognise — the classic source of a second strip. Close it
    /// rather than leaving an orphan behind.
    #[test]
    fn inject_strip_closes_the_new_pane_when_it_cannot_be_labelled() {
        struct RenameFails {
            calls: RefCell<Vec<Vec<String>>>,
        }
        impl HerdrCli for RenameFails {
            fn run_json(&self, args: &[&str]) -> io::Result<String> {
                self.calls
                    .borrow_mut()
                    .push(args.iter().map(|s| s.to_string()).collect());
                match args {
                    ["pane", "split", ..] => {
                        Ok(r#"{"result":{"pane":{"pane_id":"w1:pNEW"}}}"#.into())
                    }
                    ["pane", "rename", ..] => Err(io::Error::other("rename failed")),
                    _ => Ok(r#"{"result":{}}"#.into()),
                }
            }
        }
        let cli = RenameFails {
            calls: RefCell::new(Vec::new()),
        };
        let err = inject_strip(&cli, "w1:p1", 64, "/abs/herdr-herd", 7);
        assert!(err.is_err(), "an unlabellable strip is a failed injection");
        let calls = cli.calls.borrow();
        assert!(
            calls
                .iter()
                .any(|c| c[..2] == ["pane".to_string(), "close".to_string()] && c[2] == "w1:pNEW"),
            "the orphan pane is closed: {calls:?}"
        );
    }

    #[test]
    fn controller_strips_excludes_a_manifest_opened_herd_pane() {
        let panes = vec![
            pane("w1:p1", "w1:t1", Some("herdr-herd")),
            pane("w1:p2", "w1:t1", None),
            pane("w1:p3", "w1:t2", Some("Herd")),
        ];
        assert_eq!(controller_strips(&panes), vec!["w1:p1".to_string()]);
    }

    #[test]
    fn a_rebuilt_binary_counts_as_changed() {
        assert!(binary_changed(Some(100), Some(200)));
    }

    #[test]
    fn an_unchanged_binary_does_not_trigger_a_reload() {
        assert!(!binary_changed(Some(100), Some(100)));
    }

    /// An unreadable binary must never look like a change: a transient stat
    /// failure would otherwise restart every strip on a loop.
    #[test]
    fn an_unreadable_binary_never_triggers_a_reload() {
        assert!(!binary_changed(None, Some(200)));
        assert!(!binary_changed(Some(100), None));
        assert!(!binary_changed(None, None));
    }

    /// A reload restarts what the controller owns and can put back. A
    /// manifest-opened `Herd` pane belongs to whoever opened it, and the sweep
    /// may not be able to re-create it (a columned-bottom tab has no
    /// full-width target), so closing it would silently lose a strip.
    #[test]
    fn close_strips_restarts_the_controllers_own_strips_only() {
        let cli = FakeCli::new();
        let panes = vec![
            pane("w1:p1", "w1:t1", Some("herdr-herd")),
            pane("w1:p2", "w1:t2", Some("Herd")),
            pane("w1:p3", "w1:t2", None),
        ];
        close_strips(&cli, &panes);
        let calls = cli.calls.borrow();
        let closed: Vec<&str> = calls.iter().map(|c| c[2].as_str()).collect();
        assert_eq!(closed, vec!["w1:p1"]);
        assert!(calls.iter().all(|c| c[..2] == ["pane", "close"]));
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
            {"pane_id":"w1:p2","tab_id":"w1:t1","label":"herdr-herd"}]}}"#;
        let panes = parse_panes(j).unwrap();
        assert_eq!(panes[0], pane("w1:p1", "w1:t1", None));
        assert_eq!(panes[1], pane("w1:p2", "w1:t1", Some("herdr-herd")));
    }

    #[test]
    fn is_strip_label_matches_the_marker_and_the_manifest_title() {
        assert!(is_strip_label("herdr-herd"));
        assert!(is_strip_label("Herd"));
        assert!(!is_strip_label("claude"));
    }

    #[test]
    fn tabs_with_strip_collects_tabs_that_hold_a_marked_pane() {
        let panes = vec![
            pane("w1:p1", "w1:t1", None),
            pane("w1:p2", "w1:t1", Some("herdr-herd")),
            pane("w1:p3", "w1:t2", Some("claude")),
        ];
        let with = tabs_with_strip(&panes);
        assert!(with.contains("w1:t1"));
        assert!(!with.contains("w1:t2"), "an agent pane is not a strip");
    }

    #[test]
    fn needs_strip_is_true_for_any_tab_without_a_strip() {
        let mut with = HashSet::new();
        assert!(
            needs_strip(&tab("w1:t1", 1), &with),
            "single-pane, no strip"
        );
        assert!(
            needs_strip(&tab("w1:t2", 3), &with),
            "multi-pane is a candidate now — geometry decides per-tab"
        );
        with.insert("w1:t1".to_string());
        assert!(!needs_strip(&tab("w1:t1", 1), &with), "already has a strip");
    }

    #[test]
    fn plan_injections_includes_multi_pane_tabs_and_skips_stripped_ones() {
        let tabs = vec![tab("w1:t1", 1), tab("w1:t2", 2), tab("w1:t3", 1)];
        let panes = vec![
            pane("w1:p1", "w1:t1", None), // candidate
            pane("w1:p2", "w1:t2", None), // multi-pane, now also a candidate
            pane("w1:p9", "w1:t2", None),
            pane("w1:pA", "w1:t3", Some("Herd")), // already stripped -> skipped
        ];
        let plan = plan_injections(&tabs, &panes);
        assert_eq!(
            plan,
            vec![
                ("w1:t1".to_string(), "w1:p1".to_string()),
                ("w1:t2".to_string(), "w1:p2".to_string()),
            ]
        );
    }

    #[test]
    fn find_bottom_strip_target_picks_the_full_width_bottom_pane() {
        // Single-pane tab: the sole full-area pane qualifies.
        let single = r#"{"result":{"layout":{"area":{"x":0,"y":0,"width":100,"height":64},
            "panes":[{"pane_id":"w1:p1","rect":{"x":0,"y":0,"width":100,"height":64}}]}}}"#;
        assert_eq!(
            find_bottom_strip_target(single),
            Some(StripTarget {
                pane_id: "w1:p1".into(),
                pane_rows: 64
            })
        );

        // Top content + full-width bottom pane: the bottom pane is chosen.
        let top_bottom = full_width_bottom_layout("w1:pB");
        assert_eq!(
            find_bottom_strip_target(&top_bottom),
            Some(StripTarget {
                pane_id: "w1:pB".into(),
                pane_rows: 7
            })
        );
    }

    #[test]
    fn find_bottom_strip_target_is_none_for_a_columned_bottom_or_junk() {
        assert_eq!(find_bottom_strip_target(&columned_bottom_layout()), None);
        assert_eq!(find_bottom_strip_target("not json"), None);
        assert_eq!(
            find_bottom_strip_target(r#"{"result":{"layout":{}}}"#),
            None
        );
    }

    struct SweepFake {
        calls: RefCell<Vec<Vec<String>>>,
        tabs: String,
        panes: String,
    }
    impl HerdrCli for SweepFake {
        fn run_json(&self, args: &[&str]) -> io::Result<String> {
            self.calls
                .borrow_mut()
                .push(args.iter().map(|s| s.to_string()).collect());
            match args {
                ["tab", "list"] => Ok(self.tabs.clone()),
                ["pane", "list"] => Ok(self.panes.clone()),
                // A probe pane id containing "COL" models a columned-bottom tab
                // (no full-width bottom pane); anything else is top+bottom.
                ["pane", "layout", "--pane", id] if id.contains("COL") => {
                    Ok(columned_bottom_layout())
                }
                ["pane", "layout", "--pane", id] => Ok(full_width_bottom_layout(id)),
                ["pane", "split", ..] => Ok(r#"{"result":{"pane":{"pane_id":"w1:pNEW"}}}"#.into()),
                _ => Ok(r#"{"result":{}}"#.into()),
            }
        }
    }

    /// A recording fake that fails the `pane split` call targeting one chosen
    /// pane id (`fail_split_for`), succeeding for every other split — lets
    /// tests exercise the failure-isolation guarantees around one bad tab.
    struct FailableCli {
        calls: RefCell<Vec<Vec<String>>>,
        tabs: String,
        panes: String,
        fail_split_for: String,
    }
    impl FailableCli {
        fn new(tabs: &str, panes: &str, fail_split_for: &str) -> Self {
            FailableCli {
                calls: RefCell::new(Vec::new()),
                tabs: tabs.to_string(),
                panes: panes.to_string(),
                fail_split_for: fail_split_for.to_string(),
            }
        }
    }
    impl HerdrCli for FailableCli {
        fn run_json(&self, args: &[&str]) -> io::Result<String> {
            self.calls
                .borrow_mut()
                .push(args.iter().map(|s| s.to_string()).collect());
            match args {
                ["tab", "list"] => Ok(self.tabs.clone()),
                ["pane", "list"] => Ok(self.panes.clone()),
                ["pane", "layout", "--pane", id] => Ok(full_width_bottom_layout(id)),
                ["pane", "split", target, ..] => {
                    if *target == self.fail_split_for {
                        Err(io::Error::other("boom"))
                    } else {
                        Ok(r#"{"result":{"pane":{"pane_id":"w1:pNEW"}}}"#.into())
                    }
                }
                _ => Ok(r#"{"result":{}}"#.into()),
            }
        }
    }

    #[test]
    fn inject_strip_aborts_the_tab_without_running_or_renaming_when_split_fails() {
        let cli = FailableCli::new("", "", "w1:p1");
        let result = inject_strip(&cli, "w1:p1", 64, "/abs/herdr-herd", 7);
        assert!(result.is_err(), "a failed split must surface as an error");
        let calls = cli.calls.borrow();
        assert!(
            !calls
                .iter()
                .any(|c| c.first().map(String::as_str) == Some("pane")
                    && c.get(1).map(String::as_str) == Some("run")),
            "the render command must never run once the split has failed"
        );
        assert!(
            !calls
                .iter()
                .any(|c| c.first().map(String::as_str) == Some("pane")
                    && c.get(1).map(String::as_str) == Some("rename")),
            "the strip pane must never be renamed once the split has failed"
        );
    }

    #[test]
    fn sweep_once_continues_after_one_tab_fails() {
        // t1's split fails; t2's split must still be attempted and the sweep
        // as a whole must still report success.
        let cli = FailableCli::new(
            r#"{"result":{"tabs":[
                {"tab_id":"w1:t1","pane_count":1},
                {"tab_id":"w1:t2","pane_count":1}]}}"#,
            r#"{"result":{"panes":[
                {"pane_id":"w1:p1","tab_id":"w1:t1"},
                {"pane_id":"w1:p2","tab_id":"w1:t2"}]}}"#,
            "w1:p1",
        );
        let result = sweep_once(&cli, "/abs/herdr-herd", 7);
        assert!(
            result.is_ok(),
            "one failing tab must not abort the whole sweep"
        );
        let calls = cli.calls.borrow();
        let splits: Vec<&Vec<String>> = calls
            .iter()
            .filter(|c| {
                c.first().map(String::as_str) == Some("pane")
                    && c.get(1).map(String::as_str) == Some("split")
            })
            .collect();
        assert!(
            splits.iter().any(|c| c[2] == "w1:p2"),
            "the split for t2's pane must still be attempted after t1's split failed"
        );
    }

    #[test]
    fn sweep_once_injects_single_and_top_bottom_tabs_but_skips_columned_and_stripped() {
        // t1: single-pane, no strip           -> inject (full-width bottom).
        // t2: multi-pane, top+full-width-bottom -> inject (NEW coverage).
        // t3: single-pane, already stripped    -> skip (label de-dup).
        // t4: multi-pane, columned bottom      -> skip (no full-width bottom).
        let cli = SweepFake {
            calls: RefCell::new(Vec::new()),
            tabs: r#"{"result":{"tabs":[
                {"tab_id":"w1:t1","pane_count":1},
                {"tab_id":"w1:t2","pane_count":2},
                {"tab_id":"w1:t3","pane_count":2},
                {"tab_id":"w1:t4","pane_count":2}]}}"#
                .into(),
            panes: r#"{"result":{"panes":[
                {"pane_id":"w1:p1","tab_id":"w1:t1"},
                {"pane_id":"w1:p2","tab_id":"w1:t2"},
                {"pane_id":"w1:p9","tab_id":"w1:t2"},
                {"pane_id":"w1:pA","tab_id":"w1:t3","label":"herdr-herd"},
                {"pane_id":"w1:pB","tab_id":"w1:t3"},
                {"pane_id":"w1:pCOL","tab_id":"w1:t4"},
                {"pane_id":"w1:pD","tab_id":"w1:t4"}]}}"#
                .into(),
        };
        sweep_once(&cli, "/abs/herdr-herd", 7).unwrap();
        let calls = cli.calls.borrow();
        let split_targets: Vec<&str> = calls
            .iter()
            .filter(|c| {
                c.first().map(String::as_str) == Some("pane")
                    && c.get(1).map(String::as_str) == Some("split")
            })
            .map(|c| c[2].as_str())
            .collect();
        // t1 and t2 injected; t3 (stripped) and t4 (columned) skipped.
        assert_eq!(split_targets, vec!["w1:p1", "w1:p2"]);
    }
}
