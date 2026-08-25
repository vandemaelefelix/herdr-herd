//! Background watcher: subscribe to socket events; on any event, debounced-
//! refresh the herd (see [`crate::herdr::HerdFeed`]) and push a snapshot. A slow
//! interval refresh is the safety net; socket failure degrades to poll-only. All
//! timing is behind a clock seam so the debounce/coalesce logic is unit-testable
//! without threads.
//!
//! Events are not all equal. The "active" hat follows focus, so a focus move
//! gets a short window and everything else gets a long one. See [`Timings`] and
//! [`Debouncer`]. The debounce is leading **and** trailing: an event that lands
//! inside an open window still refreshes when the window closes, instead of
//! being dropped and leaving the herd on a stale frame until the slow poll.

use serde::Deserialize;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

use crate::agent::Agent;
use crate::herdr::HerdFeed;
use crate::socket::{SocketClient, subscribe_request};

/// A source of monotonic milliseconds, behind a seam so tests never sleep or
/// touch the real clock.
pub trait Clock {
    fn now_ms(&self) -> u64;
}

/// The real clock: milliseconds elapsed since this instance was created.
pub struct RealClock {
    origin: std::time::Instant,
}

impl RealClock {
    /// Start a new clock, with `now_ms() == 0` at the moment of creation.
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for RealClock {
    fn now_ms(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }
}

/// What a socket event line means for the watcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventClass {
    /// A focus move (`pane_focused` / `tab_focused` / `workspace_focused`). The
    /// hat marking the active agent follows focus, so this path stays fast: it
    /// is debounced only enough to coalesce the burst one switch emits.
    Focus,
    /// A tab or workspace appeared, closed, was renamed, moved or reordered:
    /// the herd changed *and* the cached breadcrumb labels are stale.
    Labels,
    /// Any other change: a pane appearing, exiting, or an agent being detected.
    Structural,
}

/// Classify one event line by its `event` name (herdr's stream names use
/// underscores: `pane_focused`, `tab_created`, …).
///
/// Anything unreadable counts as [`EventClass::Structural`], the slow window,
/// never the fast one, so junk on the wire cannot make the watcher refresh on
/// every line.
pub fn classify_event(line: &str) -> EventClass {
    #[derive(Deserialize)]
    struct EventLine {
        #[serde(default)]
        event: Option<String>,
    }
    let Some(name) = serde_json::from_str::<EventLine>(line)
        .ok()
        .and_then(|e| e.event)
    else {
        return EventClass::Structural;
    };
    if name.ends_with("_focused") {
        EventClass::Focus
    } else if name.starts_with("tab_")
        || name.starts_with("workspace_")
        || name.starts_with("worktree_")
    {
        EventClass::Labels
    } else {
        EventClass::Structural
    }
}

/// The watcher's three intervals.
///
/// The defaults are deliberately asymmetric. A refresh is cheap now that it is
/// one socket call rather than three fork/execs, but it still runs in *every*
/// herd pane at once, because the watcher subscribes to herdr's global focus
/// events: one pane switch wakes all N renderers. So the structural window is
/// long and the focus window is short, and the hat stays responsive without the
/// herd refreshing on every twitch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    /// The safety net: refresh at least this often, whatever the socket is
    /// doing. Pure status transitions (`idle`↔`working`) have no global event,
    /// so this is not really a safety net for them, it is their transport, and
    /// it bounds how stale a sheep's status can be.
    ///
    /// Kept at 2500 deliberately. #41 raised it to 5000 on the grounds that a
    /// refresh was expensive, but a refresh over the socket costs 0.7 ms of CPU
    /// against the old 25 ms, so the reason to poll slowly is gone while the
    /// cost of polling slowly (a sheep up to `slow_ms` behind reality) is not.
    pub slow_ms: u64,
    /// How long a burst of structural events is coalesced for.
    pub debounce_ms: u64,
    /// How long a burst of focus events is coalesced for. One pane switch emits
    /// up to three (`pane_focused`, `tab_focused`, `workspace_focused`); this
    /// is long enough to fold them together and short enough that the hat does
    /// not visibly trail the switch.
    pub focus_ms: u64,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            slow_ms: 2_500,
            debounce_ms: 750,
            focus_ms: 100,
        }
    }
}

/// The refresh schedule: when the next refresh is due, given what has arrived.
///
/// Split out from the loop so both the real watcher and [`drain_events`] make
/// the same decisions, and so the decisions can be tested without a thread, a
/// socket or a clock.
#[derive(Debug, Clone)]
pub struct Debouncer {
    timings: Timings,
    /// When the last snapshot was pushed; `None` before the first one.
    last_send: Option<u64>,
    /// The earliest time a pending event wants a refresh at.
    due_at: Option<u64>,
}

impl Debouncer {
    /// A schedule with nothing sent yet, so the first event refreshes at once.
    pub fn new(timings: Timings) -> Self {
        Self {
            timings,
            last_send: None,
            due_at: None,
        }
    }

    /// Record an event that arrived at `now_ms`.
    ///
    /// The refresh is scheduled for the end of this event's window, or right
    /// now if the previous refresh is already older than that. Whichever
    /// pending event wants a refresh soonest wins, so a focus move never waits
    /// out a structural window that is already open.
    pub fn on_event(&mut self, class: EventClass, now_ms: u64) {
        let window = match class {
            EventClass::Focus => self.timings.focus_ms,
            EventClass::Labels | EventClass::Structural => self.timings.debounce_ms,
        };
        let deadline = match self.last_send {
            Some(prev) => prev.saturating_add(window).max(now_ms),
            None => now_ms,
        };
        self.due_at = Some(self.due_at.map_or(deadline, |d| d.min(deadline)));
    }

    /// `true` when a refresh is due: a pending event's window has closed, or
    /// the slow-poll safety net is up.
    pub fn due(&self, now_ms: u64) -> bool {
        self.due_at.is_some_and(|d| now_ms >= d)
            || self
                .last_send
                .is_none_or(|prev| now_ms.saturating_sub(prev) >= self.timings.slow_ms)
    }

    /// Record that a refresh happened at `now_ms`, clearing anything pending.
    pub fn sent(&mut self, now_ms: u64) {
        self.last_send = Some(now_ms);
        self.due_at = None;
    }
}

/// Test seam: consume every line a socket yields, applying the same schedule
/// the real loop applies, and return the snapshots that would be pushed.
/// `event_time(i)` supplies the clock reading (ms) for the i-th event so tests
/// can place events in and out of the debounce windows.
///
/// Only the event-driven decisions: nothing here waits for a window to close on
/// its own, because the socket runs dry rather than idling.
pub fn drain_events(
    feed: &mut HerdFeed,
    socket: &mut dyn SocketClient,
    timings: Timings,
    mut event_time: impl FnMut(usize) -> u64,
) -> Vec<Vec<Agent>> {
    let _ = socket.send_line(&subscribe_request());
    let mut snaps = Vec::new();
    let mut schedule = Debouncer::new(timings);
    let mut i = 0;
    while let Ok(line) = socket.recv_line() {
        let t = event_time(i);
        i += 1;
        let class = classify_event(&line);
        if class == EventClass::Labels {
            feed.invalidate_labels();
        }
        schedule.on_event(class, t);
        if schedule.due(t) {
            if let Some(s) = feed.herd(t) {
                snaps.push(s);
            }
            schedule.sent(t);
        }
    }
    snaps
}

/// Spawn the real watcher thread. Pushes an initial snapshot, then subscribes
/// to socket events and pushes a debounced refresh on each burst. If the socket
/// is absent or errors, degrades to polling every `timings.slow_ms`; never
/// panics.
///
/// The refresh itself is [`HerdFeed`]'s problem: one `session.snapshot` on the
/// control socket, or the `herdr` CLI if that fails.
pub fn watch(
    mut feed: HerdFeed,
    mut socket: Option<Box<dyn SocketClient + Send>>,
    clock: Box<dyn Clock + Send>,
    tx: Sender<Vec<Agent>>,
    timings: Timings,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        use std::io::ErrorKind;

        let mut schedule = Debouncer::new(timings);

        // Initial snapshot.
        let now = clock.now_ms();
        if let Some(s) = feed.herd(now) {
            let _ = tx.send(s);
        }
        schedule.sent(now);

        if let Some(sock) = socket.as_mut() {
            // A failed subscribe still degrades safely: the slow poll below
            // carries the herd either way. An `eprintln!` was tried here
            // (issue #55), but this thread belongs to the render process,
            // which has the alternate screen and raw mode active — stderr
            // lands on the strip's own tty and corrupts it rather than
            // explaining anything. A real diagnostic belongs drawn into the
            // strip itself; no such surface exists yet (tracked separately
            // alongside #55/#60, see also PR #81's `sound.rs` reversion for
            // the same trap).
            let _ = sock.send_line(&subscribe_request());
        }
        loop {
            if let Some(sock) = socket.as_mut() {
                match sock.recv_line() {
                    Ok(line) => {
                        let class = classify_event(&line);
                        if class == EventClass::Labels {
                            // Only the CLI fallback caches labels, and only a
                            // tab/workspace change can stale them.
                            feed.invalidate_labels();
                        }
                        schedule.on_event(class, clock.now_ms());
                    }
                    Err(ref e)
                        if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
                    {
                        // Idle tick within the read timeout; not a failure. It
                        // is also what closes a trailing debounce window while
                        // no further events arrive.
                    }
                    Err(_) => {
                        socket = None; // real close -> degrade to poll-only
                    }
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(timings.slow_ms));
            }

            // One decision point for both the debounced refresh and the
            // slow-poll safety net, so a refresh happens at least every
            // `slow_ms` regardless of socket state (connected, idle-ticking, or
            // degraded to `None`).
            let now = clock.now_ms();
            if schedule.due(now) {
                if let Some(s) = feed.herd(now)
                    && tx.send(s).is_err()
                {
                    return;
                }
                schedule.sent(now);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
    use crate::herdr::{CommandRunner, LiveHerdr};
    use std::ffi::OsStr;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};

    const LIST: &str = r#"{"result":{"agents":[{"agent_status":"idle","cwd":"/","focused":false,"foreground_cwd":"/","pane_id":"p","revision":0,"tab_id":"t","terminal_id":"x","workspace_id":"w"}]}}"#;

    struct FakeRunner;
    impl CommandRunner for FakeRunner {
        fn run(&self, _p: &OsStr, _a: &[&str]) -> std::io::Result<Output> {
            Ok(Output {
                status: ExitStatus::from_raw(0),
                stdout: LIST.as_bytes().to_vec(),
                stderr: vec![],
            })
        }
    }

    /// A feed with no socket, so it answers from the canned CLI double.
    fn cli_feed() -> HerdFeed {
        HerdFeed::new(None, Box::new(LiveHerdr::with_runner("herdr", FakeRunner)))
    }

    // A fake socket that emits N event lines then blocks forever (returns Err).
    struct FakeSocket {
        remaining: usize,
        line: String,
    }
    impl FakeSocket {
        fn emitting(n: usize) -> Self {
            Self {
                remaining: n,
                line: r#"{"event":"pane_agent_status_changed"}"#.into(),
            }
        }
    }
    impl crate::socket::SocketClient for FakeSocket {
        fn send_line(&mut self, _l: &str) -> std::io::Result<()> {
            Ok(())
        }
        fn recv_line(&mut self) -> std::io::Result<String> {
            if self.remaining == 0 {
                return Err(std::io::Error::other("done"));
            }
            self.remaining -= 1;
            Ok(self.line.clone())
        }
    }

    fn timings(debounce_ms: u64) -> Timings {
        Timings {
            debounce_ms,
            ..Timings::default()
        }
    }

    #[test]
    fn debounce_coalesces_a_burst_into_one_refetch() {
        let mut feed = cli_feed();
        // 5 events arriving within the debounce window => 1 snapshot.
        let snaps = drain_events(
            &mut feed,
            &mut FakeSocket::emitting(5),
            timings(250),
            |_| 0, /* all same tick */
        );
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].len(), 1);
        assert_eq!(snaps[0][0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn separated_events_produce_separate_refetches() {
        let mut feed = cli_feed();
        let mut tick = 0u64;
        let snaps = drain_events(
            &mut feed,
            &mut FakeSocket::emitting(3),
            timings(250),
            move |_| {
                tick += 1000;
                tick
            },
        );
        assert_eq!(snaps.len(), 3);
    }

    #[test]
    fn focus_events_are_classified_apart_from_structural_ones() {
        assert_eq!(
            classify_event(r#"{"event":"pane_focused","data":{}}"#),
            EventClass::Focus
        );
        assert_eq!(
            classify_event(r#"{"event":"tab_focused","data":{}}"#),
            EventClass::Focus
        );
        assert_eq!(
            classify_event(r#"{"event":"workspace_focused","data":{}}"#),
            EventClass::Focus
        );
        assert_eq!(
            classify_event(r#"{"event":"pane_created","data":{}}"#),
            EventClass::Structural
        );
    }

    #[test]
    fn tab_and_workspace_changes_are_classified_as_stale_labels() {
        for name in [
            "tab_created",
            "tab_closed",
            "tab_renamed",
            "tab_moved",
            "workspace_renamed",
            "worktree_opened",
        ] {
            assert_eq!(
                classify_event(&format!(r#"{{"event":"{name}"}}"#)),
                EventClass::Labels,
                "{name} stales the breadcrumb labels"
            );
        }
    }

    /// Junk on the wire must take the slow window, never the fast one.
    #[test]
    fn an_unreadable_event_line_is_treated_as_structural() {
        assert_eq!(classify_event("not json"), EventClass::Structural);
        assert_eq!(classify_event("{}"), EventClass::Structural);
        assert_eq!(classify_event(r#"{"event":null}"#), EventClass::Structural);
    }

    /// The reason focus is classified at all: a pane switch must not wait out
    /// the long structural window before the hat moves.
    #[test]
    fn a_focus_event_refreshes_long_before_a_structural_one_would() {
        let t = Timings {
            slow_ms: 2_500,
            debounce_ms: 750,
            focus_ms: 100,
        };
        let mut structural = Debouncer::new(t);
        structural.sent(0);
        structural.on_event(EventClass::Structural, 10);
        assert!(!structural.due(700), "still inside the structural window");
        assert!(structural.due(750));

        let mut focus = Debouncer::new(t);
        focus.sent(0);
        focus.on_event(EventClass::Focus, 10);
        assert!(focus.due(100), "the hat follows within the focus window");
    }

    /// A focus move arriving while a structural window is open must pull the
    /// refresh forward, not queue behind it.
    #[test]
    fn a_focus_event_pulls_a_pending_structural_refresh_forward() {
        let mut d = Debouncer::new(Timings::default());
        d.sent(0);
        d.on_event(EventClass::Structural, 10);
        d.on_event(EventClass::Focus, 20);
        assert!(d.due(100), "the focus window is the one that applies");
    }

    /// The trailing edge: an event that lands inside an open window still gets
    /// its refresh once the window closes. Dropping it would leave the herd on
    /// a stale frame until the slow poll.
    #[test]
    fn an_event_inside_the_window_still_refreshes_when_the_window_closes() {
        let mut d = Debouncer::new(Timings::default());
        d.sent(0);
        d.on_event(EventClass::Structural, 1);
        assert!(!d.due(1), "not immediately");
        assert!(d.due(750), "but when the window closes");
        d.sent(750);
        assert!(!d.due(751), "and then it is settled");
    }

    /// The safety net carries the status transitions that have no global event.
    #[test]
    fn the_slow_poll_comes_due_with_no_events_at_all() {
        let t = Timings::default();
        let mut d = Debouncer::new(t);
        d.sent(0);
        assert!(!d.due(t.slow_ms - 1));
        assert!(d.due(t.slow_ms));
    }

    #[test]
    fn the_defaults_keep_focus_far_faster_than_the_structural_window() {
        let t = Timings::default();
        assert_eq!(t.debounce_ms, 750);
        assert_eq!(t.slow_ms, 2_500);
        assert!(
            t.focus_ms < t.debounce_ms,
            "raising the debounce must not slow the hat"
        );
    }
}
