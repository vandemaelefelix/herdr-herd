//! The controller/watchdog: keep a slim full-width herd strip in every eligible
//! tab via a non-destructive `pane split` (never `layout.apply`, which kills
//! processes — Phase 3 spike). Pure sweep logic here; the loop + I/O are thin
//! shells over the `herdr` CLI seam. See the Phase 3 design spec.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::herdr::HerdrCli;
use crate::place::slim_ratio;
use crate::snapshot::parse_session_snapshot;
use crate::socket::RpcClient;
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
    strip_target_from_layout(v.get("result")?.get("layout")?)
}

/// The same choice, made straight from a layout object.
///
/// This is the shape `session.snapshot` hands over in `layouts[]`, one entry per
/// tab, so on the socket path the controller picks every tab's split target
/// without a `pane layout` spawn per candidate.
pub fn strip_target_from_layout(layout: &Value) -> Option<StripTarget> {
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

/// `true` if a `herdr pane process-info` reply shows this pane's foreground
/// process is our renderer.
///
/// A strip pane whose renderer exited falls back to its shell but **keeps its
/// label**, so every later sweep counts the tab as covered and never re-injects.
/// Under the kitty backend the corpse is invisible: placements are only deleted
/// by `teardown` on a clean exit, so the last frame drawn — sheep, hat and all —
/// stays frozen on screen, looking like a live strip that has silently stopped
/// tracking focus.
///
/// Tolerant: anything unreadable counts as **live**, so a transient failure
/// never closes a healthy strip.
pub fn renderer_is_running(process_info_json: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(process_info_json) else {
        return true;
    };
    let Some(procs) = v
        .get("result")
        .and_then(|r| r.get("process_info"))
        .and_then(|p| p.get("foreground_processes"))
        .and_then(Value::as_array)
    else {
        return true;
    };
    if procs.is_empty() {
        return true;
    }
    procs.iter().any(|p| {
        ["name", "argv0"]
            .iter()
            .filter_map(|k| p.get(*k).and_then(Value::as_str))
            .any(|n| n.trim_start_matches('-') == RENDERER_PROCESS_NAME)
    })
}

/// The process name a running strip renderer reports — this crate's own binary.
const RENDERER_PROCESS_NAME: &str = "herdr-herd";

/// How long the controller goes between liveness probes of the same strip.
///
/// Probing every strip every sweep is the cost issue #59 names: at ten tabs and
/// the 3 s sweep floor that is ~3 process spawns a second, asking panes that
/// answered a moment ago whether they are still there. A strip that has died
/// stays dead, so a longer interval costs only latency before the replacement
/// lands, and in the common case the pane exits with its renderer anyway
/// (`inject_strip` `exec`s it), which the sweep sees for free in the pane list.
pub const STRIP_PROBE_INTERVAL_MS: u64 = 30_000;

/// How many sweeps of `interval` fit in [`STRIP_PROBE_INTERVAL_MS`]. At least
/// one, so a sweep interval longer than the probe interval still probes every
/// time round rather than dividing to zero.
pub fn probe_every_sweeps(interval: Duration) -> u64 {
    let ms = (interval.as_millis().max(1)) as u64;
    (STRIP_PROBE_INTERVAL_MS / ms).max(1)
}

/// Which strips answered "the renderer is running" recently, so a sweep only
/// pays for a `pane process-info` probe on the ones it has not checked lately.
///
/// Keyed by pane id and counted in sweeps rather than milliseconds: the
/// controller's only clock is its own loop, and counting sweeps keeps the
/// decision pure and testable.
#[derive(Debug, Default)]
pub struct StripHealth {
    confirmed: HashMap<String, u64>,
}

impl StripHealth {
    /// The strips to probe on sweep `sweep`: every one never probed (so a strip
    /// inherited from a previous controller is checked the moment it is seen),
    /// and every one last confirmed `probe_every` or more sweeps ago.
    pub fn due(&self, strips: &[String], sweep: u64, probe_every: u64) -> Vec<String> {
        strips
            .iter()
            .filter(|id| match self.confirmed.get(*id) {
                Some(last) => sweep.saturating_sub(*last) >= probe_every,
                None => true,
            })
            .cloned()
            .collect()
    }

    /// Record that `pane_id` answered "renderer running" on sweep `sweep`.
    pub fn confirm(&mut self, pane_id: &str, sweep: u64) {
        self.confirmed.insert(pane_id.to_string(), sweep);
    }

    /// Forget strips that are gone, so a controller that runs for days does not
    /// keep an entry for every pane it has ever seen.
    pub fn forget_missing(&mut self, strips: &[String]) {
        let alive: HashSet<&str> = strips.iter().map(String::as_str).collect();
        self.confirmed.retain(|id, _| alive.contains(id.as_str()));
    }

    /// The strip pane ids a previous sweep confirmed were running their
    /// renderer, for [`plan_reap`]'s liveness tiebreak.
    pub fn confirmed_ids(&self) -> HashSet<String> {
        self.confirmed.keys().cloned().collect()
    }
}

/// One sweep's view of the session.
#[derive(Debug, Default)]
pub struct SessionView {
    pub tabs: Vec<TabRef>,
    pub panes: Vec<PaneRef>,
    /// `tab_id → that tab's layout`. Filled on the socket path, where one
    /// `session.snapshot` carries every tab's layout; empty on the CLI
    /// fallback, where each candidate tab still costs its own `pane layout`.
    pub layouts: HashMap<String, Value>,
}

/// The controller, sweeping.
///
/// Owns its two sources (the control socket first, the `herdr` CLI as the
/// fallback) and the per-strip health memo, which only pays for itself if it
/// survives between sweeps.
pub struct Sweeper<'a> {
    rpc: Option<&'a dyn RpcClient>,
    cli: &'a dyn HerdrCli,
    self_exe: &'a str,
    config_dir_override: Option<&'a str>,
    target_rows: u16,
    probe_every: u64,
    sweep: u64,
    health: StripHealth,
}

impl<'a> Sweeper<'a> {
    /// Wire a sweeper to its sources. `rpc` is `None` outside a herdr session,
    /// which puts every read back on the CLI. `config_dir_override` is the
    /// controller's own `HERDR_HERD_CONFIG_DIR`, resolved once by the caller
    /// and forwarded to every strip this sweeper injects, so a test session
    /// stays isolated end to end rather than only at the controller.
    pub fn new(
        rpc: Option<&'a dyn RpcClient>,
        cli: &'a dyn HerdrCli,
        self_exe: &'a str,
        config_dir_override: Option<&'a str>,
        target_rows: u16,
        probe_every: u64,
    ) -> Self {
        Self {
            rpc,
            cli,
            self_exe,
            config_dir_override,
            target_rows,
            probe_every: probe_every.max(1),
            sweep: 0,
            health: StripHealth::default(),
        }
    }

    /// One sweep: read the session, then inject a strip into every tab that has
    /// a full-width bottom pane (single-pane tabs and top+bottom multi-pane
    /// tabs). A tab with no full-width bottom pane (columned bottom) is skipped
    /// and left to the on-demand `place`. A per-tab failure is logged and
    /// skipped so one bad tab never aborts the sweep or the others.
    pub fn sweep_once(&mut self) -> io::Result<()> {
        self.sweep = self.sweep.saturating_add(1);
        let view = self.read_session()?;
        // Reap before injecting: collapsing a tab to one strip must not be
        // undone by this same sweep deciding the tab still needs one.
        for extra in plan_reap(&view.panes, &self.health.confirmed_ids()) {
            if let Err(e) = self.cli.run_json(&["pane", "close", &extra]) {
                eprintln!("herdr-herd: could not close duplicate strip {extra}: {e}");
            }
        }
        // Close strips whose renderer has died. Left alone they keep their
        // label forever, so the tab looks covered and never gets a working
        // strip back. The next sweep injects the replacement: closing and
        // re-injecting in one pass would race the layout this sweep already
        // read.
        for dead in self.plan_dead_strips(&view.panes) {
            if let Err(e) = self.cli.run_json(&["pane", "close", &dead]) {
                eprintln!("herdr-herd: could not close dead strip {dead}: {e}");
            }
        }
        for (tab_id, probe_pane) in plan_injections(&view.tabs, &view.panes) {
            let result = (|| -> io::Result<()> {
                match self.strip_target(&view, &tab_id, &probe_pane)? {
                    Some(target) => inject_strip(
                        self.cli,
                        &target.pane_id,
                        target.pane_rows,
                        self.self_exe,
                        self.config_dir_override,
                        self.target_rows,
                    ),
                    None => Ok(()), // columned bottom: no full-width strip possible
                }
            })();
            if let Err(e) = result {
                eprintln!("herdr-herd: could not place strip in {tab_id}: {e}");
            }
        }
        Ok(())
    }

    /// The tabs, panes and (on the socket path) layouts this sweep works from.
    ///
    /// One `session.snapshot` replaces `tab list` + `pane list` + a `pane
    /// layout` per candidate tab. Anything the socket cannot answer (no
    /// socket, a failed call, a reply we cannot read) falls back to those CLI
    /// spawns rather than skipping the sweep.
    fn read_session(&self) -> io::Result<SessionView> {
        if let Some(rpc) = self.rpc
            && let Ok(reply) = rpc.call(&socket::snapshot_request())
            && let Ok(snapshot) = parse_session_snapshot(&reply)
        {
            return Ok(SessionView {
                tabs: snapshot.tabs,
                panes: snapshot.panes,
                layouts: snapshot.layouts,
            });
        }
        Ok(SessionView {
            tabs: parse_tabs(&self.cli.run_json(&["tab", "list"])?)?,
            panes: parse_panes(&self.cli.run_json(&["pane", "list"])?)?,
            layouts: HashMap::new(),
        })
    }

    /// Where to split for `tab_id`'s strip: from the snapshot's own layout when
    /// there is one, else from a `pane layout` spawn probing `probe_pane`.
    fn strip_target(
        &self,
        view: &SessionView,
        tab_id: &str,
        probe_pane: &str,
    ) -> io::Result<Option<StripTarget>> {
        match view.layouts.get(tab_id) {
            Some(layout) => Ok(strip_target_from_layout(layout)),
            None => {
                let json = self
                    .cli
                    .run_json(&["pane", "layout", "--pane", probe_pane])?;
                Ok(find_bottom_strip_target(&json))
            }
        }
    }

    /// The strip panes whose renderer is no longer running, so the sweep can
    /// close them and re-inject a live strip next time round.
    ///
    /// Only strips due a probe are asked; the rest are taken as live on the
    /// strength of their last answer (see [`StripHealth`]).
    fn plan_dead_strips(&mut self, panes: &[PaneRef]) -> Vec<String> {
        let strips = controller_strips(panes);
        self.health.forget_missing(&strips);
        let sweep = self.sweep;
        let due = self.health.due(&strips, sweep, self.probe_every);
        let mut dead = Vec::new();
        for id in due {
            // An unreadable reply counts as live, same as a malformed one:
            // never reap on doubt. It is not recorded as confirmed either, so
            // the next sweep asks again instead of trusting a non-answer.
            match self.process_info(&id) {
                Ok(reply) if !renderer_is_running(&reply) => dead.push(id),
                Ok(_) => self.health.confirm(&id, sweep),
                Err(_) => {}
            }
        }
        dead
    }

    /// One pane's foreground processes: over the control socket when it is up,
    /// else a `herdr pane process-info` spawn.
    fn process_info(&self, pane_id: &str) -> io::Result<String> {
        if let Some(rpc) = self.rpc
            && let Ok(reply) = rpc.call(&socket::process_info_request(pane_id))
        {
            return Ok(reply);
        }
        self.cli
            .run_json(&["pane", "process-info", "--pane", pane_id])
    }
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
/// strip except the one kept for that tab. Injection alone cannot guarantee
/// this — it only ever *adds* — so the sweep reaps whatever a lost label, a
/// restored session, or a `place` racing the sweep left behind.
///
/// `confirmed_live` is [`StripHealth::confirmed_ids`]: strips a previous
/// sweep's probe found actually running. Within a tab, a confirmed-live
/// strip is kept over an unconfirmed one (falling back to list order when
/// none is confirmed), so a dead-and-alive pair does not get decided by
/// coincidence of pane-list order. Deciding by order alone can reap the one
/// live strip in the same sweep [`Sweeper::plan_dead_strips`] closes the dead
/// one, leaving the tab with zero strips for a full probe interval.
pub fn plan_reap(panes: &[PaneRef], confirmed_live: &HashSet<String>) -> Vec<String> {
    let strips: Vec<&PaneRef> = panes
        .iter()
        .filter(|p| p.label.as_deref().is_some_and(is_strip_label))
        .collect();

    let mut keep: HashMap<&str, &str> = HashMap::new();
    for p in &strips {
        keep.entry(p.tab_id.as_str())
            .and_modify(|kept| {
                if !confirmed_live.contains(*kept) && confirmed_live.contains(p.pane_id.as_str()) {
                    *kept = p.pane_id.as_str();
                }
            })
            .or_insert(p.pane_id.as_str());
    }

    strips
        .into_iter()
        .filter(|p| keep.get(p.tab_id.as_str()) != Some(&p.pane_id.as_str()))
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
///
/// `config_dir_override` carries the controller's own `HERDR_HERD_CONFIG_DIR`
/// (if any) into the strip's exec line: a new pane's shell does not inherit it
/// from the controller process, so without this the strip would fall back to
/// resolving the real installed plugin's config dir instead of the isolated
/// one the controller is using.
pub fn inject_strip(
    cli: &dyn HerdrCli,
    target_pane: &str,
    pane_rows: u16,
    self_exe: &str,
    config_dir_override: Option<&str>,
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
    // `exec` so the renderer *replaces* the pane's shell: when it exits the
    // pane exits with it, rather than lingering as a labelled corpse that every
    // later sweep counts as a working strip. `pane run` executes via a shell,
    // so both `self_exe` and `config_dir` are single-quoted rather than pasted
    // in raw: an unescaped `'` in either path (e.g. `/Users/o'brien/...`)
    // would break the quoting and the renderer would silently never start.
    let render_cmd = match config_dir_override {
        Some(dir) => format!(
            "HERDR_HERD_CONFIG_DIR={} exec {} render",
            shell_single_quote(dir),
            shell_single_quote(self_exe)
        ),
        None => format!("exec {} render", shell_single_quote(self_exe)),
    };
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

/// Wrap `s` in single quotes for a POSIX shell, escaping any embedded single
/// quote as `'\''` (close the quote, an escaped literal quote, reopen it).
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
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

/// Replace this process's image, behind a seam so `reload`'s failure path is
/// testable: a real `exec` would replace the test binary itself, so it must
/// never actually run in a test.
pub trait Exec {
    /// Exec `self_exe` with the current process's own argv (skipping argv[0]).
    /// Only returns on failure — a successful exec never comes back.
    fn exec(&self, self_exe: &str) -> io::Error;
}

/// Production: `execve` over this process, preserving argv.
pub struct RealExec;

#[cfg(unix)]
impl Exec for RealExec {
    fn exec(&self, self_exe: &str) -> io::Error {
        use std::os::unix::process::CommandExt;
        let args: Vec<String> = std::env::args().skip(1).collect();
        std::process::Command::new(self_exe).args(args).exec()
    }
}

/// Run the watchdog: take the single-owner lock (exit cleanly if another
/// controller holds it), then sweep every `interval` forever. The poll unifies
/// startup, new-tab injection, and respawn/re-assert (a closed strip reappears
/// next sweep). A failed whole sweep is logged and retried next interval.
pub fn control(
    rpc: Option<&dyn RpcClient>,
    cli: &dyn HerdrCli,
    self_exe: &str,
    config_dir_override: Option<&str>,
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
    let mut sweeper = Sweeper::new(
        rpc,
        cli,
        self_exe,
        config_dir_override,
        target_rows,
        probe_every_sweeps(interval),
    );
    let exec = RealExec;
    let mut baseline = binary_stamp(self_exe);
    loop {
        control_tick(
            &mut sweeper,
            cli,
            &exec,
            self_exe,
            &mut baseline,
            binary_stamp(self_exe),
        );
        std::thread::sleep(interval);
    }
}

/// One control-loop iteration: reload if the binary changed underneath us,
/// then sweep. Split out from [`control`]'s infinite loop so tests can drive a
/// bounded number of iterations instead of the real `thread::sleep` forever;
/// `current_stamp` is this iteration's freshly read binary stamp, read by the
/// caller so a test can supply one directly without touching the filesystem.
fn control_tick(
    sweeper: &mut Sweeper,
    cli: &dyn HerdrCli,
    exec: &dyn Exec,
    self_exe: &str,
    baseline: &mut Option<u64>,
    current_stamp: Option<u64>,
) {
    if binary_changed(*baseline, current_stamp) {
        let err = reload(cli, self_exe, exec);
        // Only reached if the re-exec failed. Adopt the new stamp anyway, so a
        // binary we cannot exec does not restart the herd on every tick
        // forever, then keep sweeping: this image is stale, but the sweep
        // re-injects the strips just closed, and a stale herd beats no herd.
        eprintln!("herdr-herd: could not re-exec {self_exe}: {err}; staying on this build");
        *baseline = current_stamp;
    }
    if let Err(e) = sweeper.sweep_once() {
        eprintln!("herdr-herd: sweep failed: {e}");
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
fn reload(cli: &dyn HerdrCli, self_exe: &str, exec: &dyn Exec) -> io::Error {
    eprintln!("herdr-herd: binary changed; restarting the herd");
    match cli.run_json(&["pane", "list"]).map(|s| parse_panes(&s)) {
        Ok(Ok(panes)) => close_strips(cli, &panes),
        Ok(Err(e)) | Err(e) => {
            eprintln!("herdr-herd: could not list panes to restart them: {e}");
        }
    }
    exec.exec(self_exe)
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
    use std::rc::Rc;

    /// A sweeper with no socket (so every read goes through the CLI double) and
    /// `probe_every = 1`, i.e. probing every strip every sweep.
    fn sweeper(cli: &dyn HerdrCli) -> Sweeper<'_> {
        Sweeper::new(None, cli, "/abs/herdr-herd", None, 7, 1)
    }

    const SNAPSHOT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/session-snapshot.json"
    ));

    /// A control-socket double: answers `session.snapshot` from the fixture and
    /// `pane.process_info` per pane, recording every method it was asked for.
    struct FakeRpc {
        calls: RefCell<Vec<String>>,
        snapshot: String,
        /// Panes whose renderer has exited, so the probe reports a shell.
        dead: RefCell<HashSet<String>>,
        /// Panes the socket cannot answer for at all.
        unanswerable: HashSet<String>,
    }
    impl FakeRpc {
        fn new(snapshot: &str) -> Self {
            FakeRpc {
                calls: RefCell::new(Vec::new()),
                snapshot: snapshot.to_string(),
                dead: RefCell::new(HashSet::new()),
                unanswerable: HashSet::new(),
            }
        }
        /// The renderer in `pane_id` has just exited.
        fn kill(&self, pane_id: &str) {
            self.dead.borrow_mut().insert(pane_id.to_string());
        }
        fn calls_of(&self, method: &str) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|c| c.starts_with(method))
                .count()
        }
    }
    impl RpcClient for FakeRpc {
        fn call(&self, payload: &str) -> io::Result<String> {
            let v: Value = serde_json::from_str(payload).map_err(io::Error::other)?;
            let method = v["method"].as_str().unwrap_or_default().to_string();
            let pane = v["params"]["pane_id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            self.calls.borrow_mut().push(format!("{method} {pane}"));
            match method.as_str() {
                "session.snapshot" => Ok(self.snapshot.clone()),
                "pane.process_info" if self.unanswerable.contains(&pane) => {
                    Err(io::Error::other("no answer"))
                }
                "pane.process_info" => Ok(process_info(if self.dead.borrow().contains(&pane) {
                    "zsh"
                } else {
                    "herdr-herd"
                })),
                _ => Ok(r#"{"result":{}}"#.into()),
            }
        }
    }

    /// A CLI double for the socket-path tests: it answers the mutations
    /// (`pane split`/`run`/`rename`/`close`) and records everything, so a test
    /// can assert that no *read* ever reached it.
    fn one_tab_cli() -> SweepFake {
        SweepFake {
            calls: RefCell::new(Vec::new()),
            tabs: r#"{"result":{"tabs":[{"tab_id":"w1:t1","pane_count":2}]}}"#.into(),
            panes: r#"{"result":{"panes":[
                {"pane_id":"w1:p1","tab_id":"w1:t1"},
                {"pane_id":"w1:pLIVE","tab_id":"w1:t1","label":"herdr-herd"}]}}"#
                .into(),
        }
    }

    fn calls_matching(cli: &SweepFake, argv: &str) -> usize {
        cli.calls
            .borrow()
            .iter()
            .filter(|c| c.join(" ").starts_with(argv))
            .count()
    }

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
        inject_strip(&cli, "w1:p1", 64, "/abs/herdr-herd", None, 7).unwrap();
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
        // `exec` so the renderer replaces the pane's shell: when it exits the
        // pane exits with it, instead of lingering as a labelled corpse that
        // every later sweep counts as a working strip.
        assert_eq!(
            calls[1],
            vec!["pane", "run", "w1:pNEW", "exec '/abs/herdr-herd' render"]
        );
        assert_eq!(calls[2], vec!["pane", "rename", "w1:pNEW", "herdr-herd"]);
    }

    /// A test session's isolated config dir must reach the strip's exec line
    /// too, not just the controller: a new pane's shell does not inherit the
    /// controller process's env, so without this the strip would resolve the
    /// real installed plugin's config instead.
    #[test]
    fn inject_strip_forwards_the_config_dir_override_into_the_exec_line() {
        let cli = FakeCli::new();
        inject_strip(
            &cli,
            "w1:p1",
            64,
            "/abs/herdr-herd",
            Some("/abs/.herd-test/config"),
            7,
        )
        .unwrap();
        let calls = cli.calls.borrow();
        assert_eq!(
            calls[1],
            vec![
                "pane",
                "run",
                "w1:pNEW",
                "HERDR_HERD_CONFIG_DIR='/abs/.herd-test/config' exec '/abs/herdr-herd' render"
            ]
        );
    }

    /// `pane run` executes via a shell. An unescaped `'` in `self_exe` (e.g.
    /// a home directory like `/Users/o'brien`) would break the quoting and
    /// the renderer would silently never start.
    #[test]
    fn inject_strip_escapes_a_single_quote_in_self_exe() {
        let cli = FakeCli::new();
        inject_strip(&cli, "w1:p1", 64, "/Users/o'brien/herdr-herd", None, 7).unwrap();
        let calls = cli.calls.borrow();
        assert_eq!(
            calls[1],
            vec![
                "pane",
                "run",
                "w1:pNEW",
                r"exec '/Users/o'\''brien/herdr-herd' render"
            ]
        );
    }

    /// The config dir goes through the same escaping as `self_exe` — an
    /// unescaped `'` there would break the quoting just as badly.
    #[test]
    fn inject_strip_escapes_a_single_quote_in_the_config_dir_override() {
        let cli = FakeCli::new();
        inject_strip(
            &cli,
            "w1:p1",
            64,
            "/abs/herdr-herd",
            Some("/tmp/o'brien/.herd-test/config"),
            7,
        )
        .unwrap();
        let calls = cli.calls.borrow();
        assert_eq!(
            calls[1],
            vec![
                "pane",
                "run",
                "w1:pNEW",
                r"HERDR_HERD_CONFIG_DIR='/tmp/o'\''brien/.herd-test/config' exec '/abs/herdr-herd' render"
            ]
        );
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

    /// `herdr pane process-info` for a pane whose foreground process is the
    /// named one.
    fn process_info(name: &str) -> String {
        format!(
            r#"{{"result":{{"process_info":{{"foreground_processes":[{{"argv0":"{name}","name":"{name}","pid":1}}],"pane_id":"w1:p1"}}}}}}"#
        )
    }

    #[test]
    fn a_strip_running_the_renderer_is_live() {
        assert!(renderer_is_running(&process_info("herdr-herd")));
    }

    /// The bug this exists for: when the renderer exits, the pane falls back to
    /// a shell but keeps its label. Kitty placements are not deleted on an
    /// abnormal exit, so the last frame — sheep, hat and all — stays frozen on
    /// screen and looks like a working strip that has stopped tracking focus.
    #[test]
    fn a_strip_that_fell_back_to_a_shell_is_dead() {
        assert!(!renderer_is_running(&process_info("zsh")));
    }

    /// Never reap on a reply we could not read: a transient failure must not
    /// close a healthy strip.
    #[test]
    fn an_unreadable_process_info_counts_as_live() {
        assert!(renderer_is_running(r#"{"result":{}}"#));
        assert!(renderer_is_running("not json"));
        assert!(renderer_is_running(
            r#"{"result":{"process_info":{"foreground_processes":[]}}}"#
        ));
    }

    #[test]
    fn probe_every_sweeps_scales_with_the_sweep_interval() {
        // The 3 s default floor: one probe per strip per ten sweeps.
        assert_eq!(probe_every_sweeps(Duration::from_millis(3_000)), 10);
        assert_eq!(probe_every_sweeps(Duration::from_millis(250)), 120);
        // A sweep slower than the probe interval still probes every sweep,
        // rather than dividing to zero and probing on none of them.
        assert_eq!(probe_every_sweeps(Duration::from_secs(60)), 1);
    }

    #[test]
    fn a_strip_seen_for_the_first_time_is_probed_at_once() {
        let health = StripHealth::default();
        let strips = vec!["w1:pA".to_string()];
        assert_eq!(health.due(&strips, 1, 10), strips, "never probed before");
    }

    #[test]
    fn a_strip_confirmed_live_is_not_probed_again_until_the_interval_is_up() {
        let mut health = StripHealth::default();
        let strips = vec!["w1:pA".to_string()];
        health.confirm("w1:pA", 1);
        assert!(health.due(&strips, 10, 10).is_empty(), "9 sweeps on: quiet");
        assert_eq!(health.due(&strips, 11, 10), strips, "10 sweeps on: due");
    }

    /// A controller can run for days; the memo must not grow an entry for every
    /// pane that has ever held a strip.
    #[test]
    fn the_probe_memo_forgets_strips_that_are_gone() {
        let mut health = StripHealth::default();
        health.confirm("w1:pGONE", 1);
        health.confirm("w1:pA", 1);
        health.forget_missing(&["w1:pA".to_string()]);
        assert!(
            health.due(&["w1:pGONE".to_string()], 2, 10) == vec!["w1:pGONE".to_string()],
            "a pane that comes back is probed as if new"
        );
        assert!(health.due(&["w1:pA".to_string()], 2, 10).is_empty());
    }

    /// Issue #59: the sweep used to ask every strip, every sweep, whether it was
    /// still alive.
    #[test]
    fn a_live_strip_is_probed_once_per_interval_not_once_per_sweep() {
        let cli = one_tab_cli();
        let rpc = FakeRpc::new(SNAPSHOT);
        let mut sw = Sweeper::new(Some(&rpc), &cli, "/abs/herdr-herd", None, 7, 10);
        for _ in 0..5 {
            sw.sweep_once().unwrap();
        }
        assert_eq!(
            rpc.calls_of("pane.process_info"),
            1,
            "five sweeps, one probe"
        );

        let cli = one_tab_cli();
        let rpc = FakeRpc::new(SNAPSHOT);
        let mut every = Sweeper::new(Some(&rpc), &cli, "/abs/herdr-herd", None, 7, 1);
        for _ in 0..5 {
            every.sweep_once().unwrap();
        }
        assert_eq!(
            rpc.calls_of("pane.process_info"),
            5,
            "the old behaviour, for comparison"
        );
    }

    /// The bound on what the memo costs: a strip that dies between probes is
    /// still reaped, just on its next probe rather than the next sweep.
    #[test]
    fn a_strip_that_dies_between_probes_is_caught_on_its_next_probe() {
        let cli = one_tab_cli();
        let rpc = FakeRpc::new(SNAPSHOT);
        let mut sw = Sweeper::new(Some(&rpc), &cli, "/abs/herdr-herd", None, 7, 3);
        sw.sweep_once().unwrap(); // sweep 1: probed, alive, confirmed
        rpc.kill("w1:pSTRIP"); // the renderer exits
        sw.sweep_once().unwrap(); // sweep 2: not due
        sw.sweep_once().unwrap(); // sweep 3: not due
        assert_eq!(
            calls_matching(&cli, "pane close"),
            0,
            "nothing is reaped on the strength of a stale answer"
        );
        sw.sweep_once().unwrap(); // sweep 4: due again
        assert_eq!(
            calls_matching(&cli, "pane close w1:pSTRIP"),
            1,
            "the dead strip is closed once its probe comes round"
        );
    }

    /// The socket cannot answer, so the probe falls back to a spawn, and the
    /// spawn's answer counts, so the strip is confirmed like any other.
    #[test]
    fn a_probe_the_socket_cannot_answer_falls_back_to_a_spawn() {
        let cli = one_tab_cli();
        let mut rpc = FakeRpc::new(SNAPSHOT);
        rpc.unanswerable.insert("w1:pSTRIP".to_string());
        let mut sw = Sweeper::new(Some(&rpc), &cli, "/abs/herdr-herd", None, 7, 10);
        sw.sweep_once().unwrap();
        sw.sweep_once().unwrap();
        assert_eq!(calls_matching(&cli, "pane close"), 0, "the strip is live");
        assert_eq!(
            calls_matching(&cli, "pane process-info"),
            1,
            "one spawn, then the memo carries it"
        );
    }

    /// Never reap on doubt, and never let a non-answer count as an answer: a
    /// probe nothing can answer leaves the strip alone and asks again.
    #[test]
    fn a_probe_nothing_can_answer_is_retried_rather_than_trusted() {
        /// A CLI double that fails every `pane process-info` and otherwise
        /// behaves like [`SweepFake`].
        struct NoProcessInfo(SweepFake);
        impl HerdrCli for NoProcessInfo {
            fn run_json(&self, args: &[&str]) -> io::Result<String> {
                match args {
                    ["pane", "process-info", ..] => {
                        self.0
                            .calls
                            .borrow_mut()
                            .push(args.iter().map(|s| s.to_string()).collect());
                        Err(io::Error::other("boom"))
                    }
                    _ => self.0.run_json(args),
                }
            }
        }

        let cli = NoProcessInfo(one_tab_cli());
        let mut rpc = FakeRpc::new(SNAPSHOT);
        rpc.unanswerable.insert("w1:pSTRIP".to_string());
        let mut sw = Sweeper::new(Some(&rpc), &cli, "/abs/herdr-herd", None, 7, 10);
        sw.sweep_once().unwrap();
        sw.sweep_once().unwrap();
        assert_eq!(
            calls_matching(&cli.0, "pane close"),
            0,
            "an unanswered probe must never close a healthy strip"
        );
        assert_eq!(
            calls_matching(&cli.0, "pane process-info"),
            2,
            "a non-answer is not a confirmation, so the next sweep asks again"
        );
    }

    /// One `session.snapshot` replaces `tab list` + `pane list` + a `pane
    /// layout` per candidate tab.
    #[test]
    fn the_socket_path_reads_the_whole_session_without_a_single_spawn() {
        let cli = one_tab_cli();
        let rpc = FakeRpc::new(SNAPSHOT);
        let mut sw = Sweeper::new(Some(&rpc), &cli, "/abs/herdr-herd", None, 7, 10);
        sw.sweep_once().unwrap();

        assert_eq!(rpc.calls_of("session.snapshot"), 1);
        for read in ["tab list", "pane list", "pane layout"] {
            assert_eq!(
                calls_matching(&cli, read),
                0,
                "`{read}` must not be spawned when the socket answered"
            );
        }
        // The fixture: t1 already has a strip, t2 has a full-width bottom pane,
        // tCOL has a columned bottom. Only t2 gets one, and its split target
        // came from the snapshot's own layout.
        let splits: Vec<String> = cli
            .calls
            .borrow()
            .iter()
            .filter(|c| c[..2] == ["pane", "split"])
            .map(|c| c[2].clone())
            .collect();
        assert_eq!(splits, vec!["w1:p3".to_string()]);
    }

    #[test]
    fn a_socket_that_cannot_answer_falls_back_to_the_cli_reads() {
        struct DeadRpc;
        impl RpcClient for DeadRpc {
            fn call(&self, _payload: &str) -> io::Result<String> {
                Err(io::Error::other("socket down"))
            }
        }
        let cli = SweepFake {
            calls: RefCell::new(Vec::new()),
            tabs: r#"{"result":{"tabs":[{"tab_id":"w1:t1","pane_count":1}]}}"#.into(),
            panes: r#"{"result":{"panes":[{"pane_id":"w1:p1","tab_id":"w1:t1"}]}}"#.into(),
        };
        Sweeper::new(Some(&DeadRpc), &cli, "/abs/herdr-herd", None, 7, 10)
            .sweep_once()
            .unwrap();
        assert_eq!(calls_matching(&cli, "tab list"), 1);
        assert_eq!(calls_matching(&cli, "pane list"), 1);
        assert_eq!(
            calls_matching(&cli, "pane layout"),
            1,
            "no snapshot layouts, so the candidate tab is probed the old way"
        );
        assert_eq!(
            calls_matching(&cli, "pane split"),
            1,
            "the strip still lands"
        );
    }

    #[test]
    fn sweep_closes_a_strip_whose_renderer_has_died() {
        let cli = SweepFake {
            calls: RefCell::new(Vec::new()),
            tabs: r#"{"result":{"tabs":[{"tab_id":"w1:t1","pane_count":2}]}}"#.into(),
            panes: r#"{"result":{"panes":[
                {"pane_id":"w1:p1","tab_id":"w1:t1"},
                {"pane_id":"w1:pDEAD","tab_id":"w1:t1","label":"herdr-herd"}]}}"#
                .into(),
        };
        sweeper(&cli).sweep_once().unwrap();
        let calls = cli.calls.borrow();
        let closes: Vec<&str> = calls
            .iter()
            .filter(|c| c[..2] == ["pane", "close"])
            .map(|c| c[2].as_str())
            .collect();
        assert_eq!(closes, vec!["w1:pDEAD"], "the dead strip is closed");
    }

    #[test]
    fn sweep_leaves_a_live_strip_running() {
        let cli = SweepFake {
            calls: RefCell::new(Vec::new()),
            tabs: r#"{"result":{"tabs":[{"tab_id":"w1:t1","pane_count":2}]}}"#.into(),
            panes: r#"{"result":{"panes":[
                {"pane_id":"w1:p1","tab_id":"w1:t1"},
                {"pane_id":"w1:p2","tab_id":"w1:t1","label":"herdr-herd"}]}}"#
                .into(),
        };
        sweeper(&cli).sweep_once().unwrap();
        let calls = cli.calls.borrow();
        assert!(
            !calls.iter().any(|c| c[..2] == ["pane", "close"]),
            "a live strip must never be closed: {calls:?}"
        );
    }

    #[test]
    fn a_tab_with_one_strip_has_nothing_to_reap() {
        let panes = vec![
            pane("w1:p1", "w1:t1", None),
            pane("w1:p2", "w1:t1", Some("herdr-herd")),
        ];
        assert!(plan_reap(&panes, &HashSet::new()).is_empty());
    }

    /// The invariant: one strip per tab, always. Whatever produced the second
    /// one (a lost label, a restored session, a `place` racing the sweep), the
    /// next sweep collapses it back to one. With no liveness info at all, the
    /// tiebreak falls back to keeping whichever came first.
    #[test]
    fn a_tab_with_two_strips_reaps_all_but_the_first() {
        let panes = vec![
            pane("w1:p1", "w1:t1", Some("herdr-herd")),
            pane("w1:p2", "w1:t1", Some("Herd")),
            pane("w1:p3", "w1:t1", Some("herdr-herd")),
        ];
        assert_eq!(
            plan_reap(&panes, &HashSet::new()),
            vec!["w1:p2".to_string(), "w1:p3".into()]
        );
    }

    /// A previous sweep's probe found `p1` dead and `p2` alive. Reaping must
    /// not undo that by closing the confirmed-live strip on the strength of
    /// list order alone — that would leave the tab with zero strips until the
    /// next probe interval.
    #[test]
    fn a_tab_with_two_strips_keeps_the_confirmed_live_one_even_if_it_is_not_first() {
        let panes = vec![
            pane("w1:p1", "w1:t1", Some("herdr-herd")),
            pane("w1:p2", "w1:t1", Some("herdr-herd")),
        ];
        let confirmed_live: HashSet<String> = ["w1:p2".to_string()].into_iter().collect();
        assert_eq!(
            plan_reap(&panes, &confirmed_live),
            vec!["w1:p1".to_string()],
            "p1 is unconfirmed and p2 is confirmed live, so p1 is the one to close"
        );
    }

    #[test]
    fn reaping_is_per_tab_so_one_strip_each_across_tabs_is_untouched() {
        let panes = vec![
            pane("w1:p1", "w1:t1", Some("herdr-herd")),
            pane("w1:p2", "w1:t2", Some("herdr-herd")),
        ];
        assert!(plan_reap(&panes, &HashSet::new()).is_empty());
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
        sweeper(&cli).sweep_once().unwrap();
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
        let err = inject_strip(&cli, "w1:p1", 64, "/abs/herdr-herd", None, 7);
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
                // A pane id containing "DEAD" models a strip whose renderer
                // exited and left the pane back at its shell.
                ["pane", "process-info", "--pane", id] if id.contains("DEAD") => {
                    Ok(process_info("zsh"))
                }
                ["pane", "process-info", ..] => Ok(process_info("herdr-herd")),
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
        let result = inject_strip(&cli, "w1:p1", 64, "/abs/herdr-herd", None, 7);
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
        let result = sweeper(&cli).sweep_once();
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
        sweeper(&cli).sweep_once().unwrap();
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

    // ---- issue #49: control()/reload()'s lifecycle -----------------------

    #[test]
    fn sweep_once_propagates_an_error_when_tab_list_fails() {
        struct FailingTabList;
        impl HerdrCli for FailingTabList {
            fn run_json(&self, args: &[&str]) -> io::Result<String> {
                match args {
                    ["tab", "list"] => Err(io::Error::other("boom")),
                    _ => Ok(r#"{"result":{"panes":[]}}"#.into()),
                }
            }
        }
        assert!(
            sweeper(&FailingTabList).sweep_once().is_err(),
            "a failed tab list must surface, not be swallowed"
        );
    }

    #[test]
    fn sweep_once_propagates_an_error_when_pane_list_fails() {
        struct FailingPaneList;
        impl HerdrCli for FailingPaneList {
            fn run_json(&self, args: &[&str]) -> io::Result<String> {
                match args {
                    ["tab", "list"] => Ok(r#"{"result":{"tabs":[]}}"#.into()),
                    ["pane", "list"] => Err(io::Error::other("boom")),
                    _ => Ok(r#"{"result":{}}"#.into()),
                }
            }
        }
        assert!(
            sweeper(&FailingPaneList).sweep_once().is_err(),
            "a failed pane list must surface, not be swallowed"
        );
    }

    #[test]
    fn control_exits_cleanly_when_another_controller_already_holds_the_lock() {
        let lock_path = std::env::temp_dir().join(format!(
            "herdr-herd-control-test-lock-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&lock_path);
        let _held = lock::acquire(&lock_path)
            .unwrap()
            .expect("this test holds the lock first");

        let cli = FakeCli::new();
        let result = control(
            None,
            &cli,
            "/abs/herdr-herd",
            None,
            &lock_path,
            Duration::from_secs(1),
            7,
        );
        assert!(
            result.is_ok(),
            "a lock contention exit must be a clean Ok(()), not an error"
        );
        assert!(
            cli.calls.borrow().is_empty(),
            "no sweep work happens when the lock is contended"
        );

        drop(_held);
        let _ = std::fs::remove_file(&lock_path);
    }

    /// An `Exec` double that always fails (a real success would replace the
    /// test binary), recording every attempt.
    struct FailingExec {
        calls: RefCell<usize>,
    }
    impl Exec for FailingExec {
        fn exec(&self, _self_exe: &str) -> io::Error {
            *self.calls.borrow_mut() += 1;
            io::Error::other("exec unavailable in tests")
        }
    }

    #[test]
    fn control_tick_reloads_and_adopts_the_new_stamp_when_the_exec_fails() {
        let cli = one_tab_cli();
        let exec = FailingExec {
            calls: RefCell::new(0),
        };
        let mut sw = sweeper(&cli);
        let mut baseline = Some(100);

        control_tick(
            &mut sw,
            &cli,
            &exec,
            "/abs/herdr-herd",
            &mut baseline,
            Some(200),
        );

        assert_eq!(
            *exec.calls.borrow(),
            1,
            "a changed stamp must trigger exactly one reload attempt"
        );
        assert_eq!(
            baseline,
            Some(200),
            "the new stamp is adopted even though the exec failed, so a binary \
             that cannot be exec'd does not retry every tick forever"
        );
    }

    #[test]
    fn control_tick_does_not_reload_when_the_stamp_is_unchanged() {
        let cli = one_tab_cli();
        let exec = FailingExec {
            calls: RefCell::new(0),
        };
        let mut sw = sweeper(&cli);
        let mut baseline = Some(100);

        control_tick(
            &mut sw,
            &cli,
            &exec,
            "/abs/herdr-herd",
            &mut baseline,
            Some(100),
        );

        assert_eq!(
            *exec.calls.borrow(),
            0,
            "an unchanged stamp reloads nothing"
        );
        assert_eq!(baseline, Some(100));
    }

    #[test]
    fn control_tick_still_sweeps_after_a_failed_reload() {
        let cli = one_tab_cli();
        let exec = FailingExec {
            calls: RefCell::new(0),
        };
        let mut sw = sweeper(&cli);
        let mut baseline = Some(1);

        control_tick(
            &mut sw,
            &cli,
            &exec,
            "/abs/herdr-herd",
            &mut baseline,
            Some(2),
        );

        // one_tab_cli's tab has no full-width-bottom candidate probed here
        // (it already holds a strip), so the sweep itself is a no-op; what
        // matters is that it ran at all rather than being skipped after the
        // reload failure.
        assert!(
            cli.calls
                .borrow()
                .iter()
                .any(|c| c[..2] == ["tab".to_string(), "list".to_string()]),
            "the sweep must still run in the same tick as a failed reload"
        );
    }

    #[test]
    fn control_tick_reload_closes_strips_before_attempting_the_reexec() {
        struct OrderedCli {
            log: Rc<RefCell<Vec<String>>>,
        }
        impl HerdrCli for OrderedCli {
            fn run_json(&self, args: &[&str]) -> io::Result<String> {
                self.log
                    .borrow_mut()
                    .push(format!("cli:{}", args.join(" ")));
                match args {
                    ["pane", "list"] => Ok(r#"{"result":{"panes":[
                        {"pane_id":"w1:p1","tab_id":"w1:t1","label":"herdr-herd"}]}}"#
                        .into()),
                    ["tab", "list"] => Ok(r#"{"result":{"tabs":[]}}"#.into()),
                    _ => Ok(r#"{"result":{}}"#.into()),
                }
            }
        }
        struct OrderedExec {
            log: Rc<RefCell<Vec<String>>>,
        }
        impl Exec for OrderedExec {
            fn exec(&self, _self_exe: &str) -> io::Error {
                self.log.borrow_mut().push("exec".into());
                io::Error::other("boom")
            }
        }

        let log = Rc::new(RefCell::new(Vec::new()));
        let cli = OrderedCli {
            log: Rc::clone(&log),
        };
        let exec = OrderedExec {
            log: Rc::clone(&log),
        };
        let mut sw = sweeper(&cli);
        let mut baseline = Some(1);

        control_tick(
            &mut sw,
            &cli,
            &exec,
            "/abs/herdr-herd",
            &mut baseline,
            Some(2),
        );

        let log = log.borrow();
        let exec_pos = log
            .iter()
            .position(|e| e == "exec")
            .expect("exec attempted");
        let close_pos = log
            .iter()
            .position(|e| e == "cli:pane close w1:p1")
            .expect("the labelled strip was closed");
        assert!(
            close_pos < exec_pos,
            "strips must be closed before the re-exec is attempted: {log:?}"
        );
    }
}
