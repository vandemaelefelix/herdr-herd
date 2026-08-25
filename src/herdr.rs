//! herdr query seam: ask herdr for the herd's state, behind traits so tests
//! never spawn a real process or touch a real socket. Ported from the
//! herdr-file-viewer plugin's pattern (unix-only here; platforms exclude
//! Windows).
//!
//! Two paths, in preference order (see [`HerdFeed`]):
//!
//! 1. the control socket, one `session.snapshot` per refresh; and
//! 2. the `herdr` CLI (`HerdrCli`/`LiveHerdr`), which fork/execs, as the
//!    fallback for when the socket is absent or failing.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::process::{Command, Output};

use crate::agent::{Agent, parse_agent_list};
use crate::sidebar::{parse_tab_labels, parse_workspace_labels};
use crate::snapshot::parse_session_snapshot;
use crate::socket::{RpcClient, snapshot_request};

/// The substitution point the app depends on: run a herdr subcommand expected
/// to emit JSON on stdout.
pub trait HerdrCli {
    fn run_json(&self, args: &[&str]) -> io::Result<String>;
}

/// Inner seam: lets tests assert argv without real spawning.
pub trait CommandRunner {
    fn run(&self, program: &OsStr, args: &[&str]) -> io::Result<Output>;
}

/// Real command execution via `std::process::Command`.
pub struct RealRunner;
impl CommandRunner for RealRunner {
    fn run(&self, program: &OsStr, args: &[&str]) -> io::Result<Output> {
        Command::new(program).args(args).output()
    }
}

/// The live herdr adapter.
pub struct LiveHerdr<R: CommandRunner = RealRunner> {
    program: OsString,
    runner: R,
}

impl LiveHerdr<RealRunner> {
    /// Resolve `herdr` from `$HERDR_BIN_PATH` (or `"herdr"` on PATH).
    pub fn from_env() -> Self {
        Self {
            program: resolve_program(std::env::var("HERDR_BIN_PATH").ok()),
            runner: RealRunner,
        }
    }
}

impl<R: CommandRunner> LiveHerdr<R> {
    pub fn with_runner(program: impl Into<OsString>, runner: R) -> Self {
        Self {
            program: program.into(),
            runner,
        }
    }
}

impl<R: CommandRunner> HerdrCli for LiveHerdr<R> {
    fn run_json(&self, args: &[&str]) -> io::Result<String> {
        let out = self.runner.run(&self.program, args)?;
        if !out.status.success() {
            return Err(io::Error::other("herdr exited non-zero"));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// How long the CLI fallback trusts a fetched label map before refetching it.
///
/// Only the fallback needs this: a `session.snapshot` carries the labels
/// alongside the agents they label, so the socket path is always consistent and
/// never caches. On the CLI path the labels cost a fork/exec each, and tab and
/// workspace names change far more slowly than agent status, so half a minute of
/// staleness buys two thirds of the refresh cost back.
pub const LABEL_TTL_MS: u64 = 30_000;

/// Where the herd's current state comes from.
///
/// Preferred path: one `session.snapshot` on the herdr control socket, which
/// carries the agents **and** the workspace/tab labels in a single reply.
///
/// Fallback path, a hard requirement because a socket failure must degrade the
/// herd rather than break it: the `herdr` CLI. `agent list` every refresh, with the two
/// label lists cached for [`LABEL_TTL_MS`], so the degraded path costs one
/// fork/exec per refresh instead of three.
///
/// A socket failure is never permanent: every refresh tries the socket first, so
/// a herdr restart is picked up on the next refresh rather than leaving the herd
/// on the CLI until the pane is restarted.
pub struct HerdFeed {
    rpc: Option<Box<dyn RpcClient + Send>>,
    cli: Box<dyn HerdrCli + Send>,
    labels: Option<CachedLabels>,
}

/// The label maps the CLI fallback reuses between refreshes, with the reading of
/// the clock they were fetched at.
struct CachedLabels {
    fetched_ms: u64,
    workspaces: HashMap<String, String>,
    tabs: HashMap<String, String>,
}

impl HerdFeed {
    /// Wire a feed to its two sources. `rpc` is `None` outside a herdr session,
    /// which puts the feed permanently on the CLI path.
    pub fn new(rpc: Option<Box<dyn RpcClient + Send>>, cli: Box<dyn HerdrCli + Send>) -> Self {
        Self {
            rpc,
            cli,
            labels: None,
        }
    }

    /// The current herd, every agent's `hover_label` resolved. `None` when both
    /// paths failed, so the caller can silently skip a bad refresh rather than
    /// push an empty herd it does not believe in.
    ///
    /// `now_ms` is a reading of the watcher's clock, used only to age the CLI
    /// fallback's label cache.
    pub fn herd(&mut self, now_ms: u64) -> Option<Vec<Agent>> {
        self.via_socket().or_else(|| self.via_cli(now_ms))
    }

    /// Drop the cached label maps, so the next CLI-path refresh refetches them.
    /// The watcher calls this when herdr reports a tab or workspace appearing,
    /// closing or being renamed: the only events that can stale a breadcrumb.
    pub fn invalidate_labels(&mut self) {
        self.labels = None;
    }

    /// One `session.snapshot` call. `None` on a missing socket, a failed call,
    /// or a reply we cannot read (including an error reply, which has no
    /// `result.snapshot` and so must not be mistaken for an empty herd).
    fn via_socket(&self) -> Option<Vec<Agent>> {
        let reply = self.rpc.as_ref()?.call(&snapshot_request()).ok()?;
        let mut snapshot = parse_session_snapshot(&reply).ok()?;
        snapshot.resolve_hover_labels();
        Some(snapshot.agents)
    }

    /// The three-spawn path, minus the two spawns the label cache absorbs.
    fn via_cli(&mut self, now_ms: u64) -> Option<Vec<Agent>> {
        let mut agents = self
            .cli
            .run_json(&["agent", "list"])
            .ok()
            .and_then(|s| parse_agent_list(&s).ok())?;
        self.refresh_labels(now_ms);
        let labels = self.labels.as_ref();
        for a in agents.iter_mut() {
            let ws = labels
                .and_then(|l| l.workspaces.get(&a.workspace_id))
                .map(String::as_str);
            let tab = labels
                .and_then(|l| l.tabs.get(&a.tab_id))
                .map(String::as_str);
            a.hover_label = Some(a.sidebar_label(ws, tab));
        }
        Some(agents)
    }

    /// Refetch the label maps if they are missing or older than
    /// [`LABEL_TTL_MS`]. A list that fails to fetch is not cached: the previous
    /// maps (or none) are kept and retried next refresh, so one bad spawn cannot
    /// blank every breadcrumb for half a minute.
    fn refresh_labels(&mut self, now_ms: u64) {
        let fresh = self
            .labels
            .as_ref()
            .is_some_and(|c| now_ms.saturating_sub(c.fetched_ms) < LABEL_TTL_MS);
        if fresh {
            return;
        }
        let workspaces = self
            .cli
            .run_json(&["workspace", "list"])
            .ok()
            .map(|s| parse_workspace_labels(&s));
        let tabs = self
            .cli
            .run_json(&["tab", "list"])
            .ok()
            .map(|s| parse_tab_labels(&s));
        let (Some(workspaces), Some(tabs)) = (workspaces, tabs) else {
            return;
        };
        self.labels = Some(CachedLabels {
            fetched_ms: now_ms,
            workspaces,
            tabs,
        });
    }
}

/// `Some(non-empty)` → that path; `None`/empty → `"herdr"`.
pub fn resolve_program(var: Option<String>) -> OsString {
    match var {
        Some(v) if !v.is_empty() => OsString::from(v),
        _ => OsString::from("herdr"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    struct Fake {
        stdout: String,
        raw_status: i32,
    }
    impl CommandRunner for Fake {
        fn run(&self, _program: &OsStr, _args: &[&str]) -> io::Result<Output> {
            Ok(Output {
                status: ExitStatus::from_raw(self.raw_status),
                stdout: self.stdout.clone().into_bytes(),
                stderr: Vec::new(),
            })
        }
    }

    #[test]
    fn run_json_returns_stdout_on_success() {
        let h = LiveHerdr::with_runner(
            "herdr",
            Fake {
                stdout: r#"{"ok":true}"#.into(),
                raw_status: 0,
            },
        );
        assert_eq!(h.run_json(&["agent", "list"]).unwrap(), r#"{"ok":true}"#);
    }

    #[test]
    fn run_json_errors_on_nonzero_exit() {
        // from_raw(256) => exit code 1 on unix.
        let h = LiveHerdr::with_runner(
            "herdr",
            Fake {
                stdout: String::new(),
                raw_status: 256,
            },
        );
        assert!(h.run_json(&["agent", "list"]).is_err());
    }

    const SNAPSHOT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/session-snapshot.json"
    ));

    /// Lets a test hold on to its double while the feed owns it.
    impl<T: HerdrCli + ?Sized> HerdrCli for std::sync::Arc<T> {
        fn run_json(&self, args: &[&str]) -> io::Result<String> {
            (**self).run_json(args)
        }
    }

    /// A CLI double that records every argv it is asked to run (joined by
    /// spaces) and answers each list with a one-entry envelope. `fail` names the
    /// one argv that errors instead.
    #[derive(Default)]
    struct RecordingCli {
        calls: std::sync::Mutex<Vec<String>>,
        fail: Option<&'static str>,
    }
    impl RecordingCli {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().map(|c| c.clone()).unwrap_or_default()
        }
        /// How many spawns this double was asked for in total.
        fn spawns(&self) -> usize {
            self.calls().len()
        }
        fn spawns_of(&self, argv: &str) -> usize {
            self.calls().iter().filter(|c| *c == argv).count()
        }
    }
    impl HerdrCli for RecordingCli {
        fn run_json(&self, args: &[&str]) -> io::Result<String> {
            let argv = args.join(" ");
            if let Ok(mut c) = self.calls.lock() {
                c.push(argv.clone());
            }
            if self.fail == Some(argv.as_str()) {
                return Err(io::Error::other("boom"));
            }
            match args {
                ["agent", "list"] => Ok(r#"{"result":{"agents":[{"agent_status":"idle","cwd":"/","focused":false,"foreground_cwd":"/","pane_id":"p","revision":0,"tab_id":"t","terminal_id":"x","workspace_id":"w"}]}}"#.into()),
                ["workspace", "list"] => {
                    Ok(r#"{"result":{"workspaces":[{"workspace_id":"w","label":"ws"}]}}"#.into())
                }
                ["tab", "list"] => Ok(r#"{"result":{"tabs":[{"tab_id":"t","label":"tab"}]}}"#.into()),
                _ => Ok(r#"{"result":{}}"#.into()),
            }
        }
    }

    /// A socket double: answers with `reply`, or fails for the first
    /// `fail_first` calls.
    struct FakeRpc {
        reply: String,
        fail_first: std::sync::Mutex<usize>,
    }
    impl FakeRpc {
        fn answering(reply: &str) -> Self {
            Self {
                reply: reply.to_string(),
                fail_first: std::sync::Mutex::new(0),
            }
        }
        fn failing_first(reply: &str, n: usize) -> Self {
            let f = Self::answering(reply);
            if let Ok(mut g) = f.fail_first.lock() {
                *g = n;
            }
            f
        }
    }
    impl RpcClient for FakeRpc {
        fn call(&self, _payload: &str) -> io::Result<String> {
            if let Ok(mut f) = self.fail_first.lock()
                && *f > 0
            {
                *f -= 1;
                return Err(io::Error::other("socket down"));
            }
            Ok(self.reply.clone())
        }
    }

    /// A feed on both paths, plus a handle on the CLI double.
    fn feed_with(
        rpc: Option<FakeRpc>,
        cli: RecordingCli,
    ) -> (HerdFeed, std::sync::Arc<RecordingCli>) {
        let cli = std::sync::Arc::new(cli);
        let rpc = rpc.map(|r| Box::new(r) as Box<dyn RpcClient + Send>);
        (
            HerdFeed::new(rpc, Box::new(std::sync::Arc::clone(&cli))),
            cli,
        )
    }

    #[test]
    fn the_socket_answers_the_refresh_and_the_cli_is_never_spawned() {
        let (mut feed, cli) =
            feed_with(Some(FakeRpc::answering(SNAPSHOT)), RecordingCli::default());
        let agents = feed.herd(0).expect("the socket answered");
        assert_eq!(agents.len(), 3);
        assert_eq!(
            agents[0].display_label(),
            "herdr-herd › renderer",
            "the snapshot resolves its own breadcrumbs"
        );
        assert_eq!(
            cli.spawns(),
            0,
            "the whole point: no fork/exec when the socket answers"
        );
    }

    #[test]
    fn a_failing_socket_degrades_to_the_cli_rather_than_breaking_the_herd() {
        let (mut feed, cli) = feed_with(
            Some(FakeRpc::failing_first(SNAPSHOT, 1)),
            RecordingCli::default(),
        );
        let agents = feed.herd(0).expect("the CLI answered");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].display_label(), "ws › tab");
        assert_eq!(cli.spawns(), 3, "agent list + workspace list + tab list");
    }

    /// An error reply parses as JSON but has no `result.snapshot`; it must not
    /// be mistaken for "the session has no agents".
    #[test]
    fn a_reply_the_socket_cannot_answer_degrades_to_the_cli() {
        let (mut feed, cli) = feed_with(
            Some(FakeRpc::answering(
                r#"{"id":"herd:snapshot","error":{"message":"unknown method"}}"#,
            )),
            RecordingCli::default(),
        );
        assert_eq!(feed.herd(0).expect("the CLI answered").len(), 1);
        assert_eq!(cli.spawns(), 3);
    }

    /// herdr restarts. One failed call must not pin the herd to the CLI for the
    /// rest of the pane's life.
    #[test]
    fn the_socket_is_retried_after_a_failure() {
        let (mut feed, _cli) = feed_with(
            Some(FakeRpc::failing_first(SNAPSHOT, 1)),
            RecordingCli::default(),
        );
        assert_eq!(feed.herd(0).expect("cli").len(), 1, "first refresh: CLI");
        assert_eq!(
            feed.herd(1).expect("socket").len(),
            3,
            "the second refresh is back on the socket"
        );
    }

    #[test]
    fn without_a_socket_the_feed_stays_on_the_cli() {
        let (mut feed, cli) = feed_with(None, RecordingCli::default());
        assert_eq!(feed.herd(0).expect("cli").len(), 1);
        assert_eq!(cli.spawns_of("agent list"), 1);
    }

    #[test]
    fn the_cli_fallback_fetches_the_label_lists_once_and_reuses_them() {
        let (mut feed, cli) = feed_with(None, RecordingCli::default());
        for t in [0, 1_000, LABEL_TTL_MS - 1] {
            assert_eq!(
                feed.herd(t).expect("cli")[0].display_label(),
                "ws › tab",
                "the cached labels still resolve the breadcrumb"
            );
        }
        assert_eq!(cli.spawns_of("agent list"), 3, "one per refresh");
        assert_eq!(
            cli.spawns_of("workspace list"),
            1,
            "cached across refreshes"
        );
        assert_eq!(cli.spawns_of("tab list"), 1, "cached across refreshes");
    }

    #[test]
    fn the_cli_fallback_refetches_the_labels_once_the_cache_has_aged_out() {
        let (mut feed, cli) = feed_with(None, RecordingCli::default());
        feed.herd(0);
        feed.herd(LABEL_TTL_MS);
        assert_eq!(cli.spawns_of("workspace list"), 2);
        assert_eq!(cli.spawns_of("tab list"), 2);
    }

    /// The watcher drops the cache when herdr reports a tab or workspace change,
    /// so a rename shows up without waiting out the TTL.
    #[test]
    fn invalidating_the_labels_refetches_them_on_the_next_refresh() {
        let (mut feed, cli) = feed_with(None, RecordingCli::default());
        feed.herd(0);
        feed.invalidate_labels();
        feed.herd(1);
        assert_eq!(cli.spawns_of("workspace list"), 2);
    }

    /// A transient label-list failure must not be cached, or one bad spawn
    /// blanks every breadcrumb for the whole TTL.
    #[test]
    fn a_failed_label_list_is_retried_rather_than_cached() {
        let (mut feed, cli) = feed_with(
            None,
            RecordingCli {
                fail: Some("workspace list"),
                ..Default::default()
            },
        );
        let agents = feed.herd(0).expect("agent list still answered");
        assert_eq!(
            agents[0].display_label(),
            "p",
            "no labels to join, so the breadcrumb degrades: no folder, so the legacy label"
        );
        feed.herd(1);
        assert_eq!(
            cli.spawns_of("workspace list"),
            2,
            "the failure was not cached, so the next refresh retries"
        );
    }

    #[test]
    fn a_refresh_neither_path_can_answer_yields_no_snapshot() {
        let (mut feed, _cli) = feed_with(
            Some(FakeRpc::answering("not json")),
            RecordingCli {
                fail: Some("agent list"),
                ..Default::default()
            },
        );
        assert!(feed.herd(0).is_none());
    }

    #[test]
    fn resolve_program_falls_back_to_herdr() {
        assert_eq!(resolve_program(None), OsString::from("herdr"));
        assert_eq!(
            resolve_program(Some(String::new())),
            OsString::from("herdr")
        );
        assert_eq!(
            resolve_program(Some("/custom/herdr".into())),
            OsString::from("/custom/herdr")
        );
    }
}
