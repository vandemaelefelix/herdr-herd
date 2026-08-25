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
/// Split out from the loop so the debounce/coalesce decisions can be tested
/// directly — without a thread, a socket or a clock — independently of
/// [`watch`], which drives the same schedule against the real socket/clock.
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

    // ---- issue #49: drive `watch` itself, not a test-only shadow ---------

    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    /// A deterministic, `Send` clock for driving `watch` on its own spawned
    /// thread: every call advances by a fixed `step`, so a test can reason
    /// about exactly which `now_ms` reading backs each loop decision without
    /// touching the real clock.
    struct StepClock {
        ms: AtomicU64,
        step: u64,
    }

    impl StepClock {
        fn new(step: u64) -> Self {
            Self {
                ms: AtomicU64::new(0),
                step,
            }
        }
    }

    impl Clock for StepClock {
        fn now_ms(&self) -> u64 {
            self.ms.fetch_add(self.step, Ordering::SeqCst)
        }
    }

    /// One scripted `recv_line` outcome for [`ScriptedSocket`].
    #[derive(Clone)]
    enum SocketOutcome {
        /// A framed event line arrives.
        Line(&'static str),
        /// `WouldBlock`: the real idle tick within the read timeout, not a
        /// dead connection (see [`SocketClient::recv_line`]).
        Idle,
        /// A real close: the production loop must degrade to poll-only.
        Closed,
    }

    /// A `SocketClient` double that drives `watch` directly, per issue #49,
    /// replacing `drain_events`, a test-only reimplementation of the debounce
    /// rule that could drift from the real loop. Yields each scripted outcome
    /// in order; once exhausted, settles into permanent idle ticks, which is
    /// always safe (never a hang risk) unlike defaulting to a close, which
    /// would route every exhausted script into the real `thread::sleep`
    /// degrade path.
    struct ScriptedSocket {
        script: std::collections::VecDeque<SocketOutcome>,
    }

    impl ScriptedSocket {
        fn new(script: Vec<SocketOutcome>) -> Self {
            Self {
                script: script.into(),
            }
        }
    }

    impl crate::socket::SocketClient for ScriptedSocket {
        fn send_line(&mut self, _line: &str) -> std::io::Result<()> {
            Ok(())
        }
        fn recv_line(&mut self) -> std::io::Result<String> {
            match self.script.pop_front() {
                Some(SocketOutcome::Line(l)) => Ok(l.to_string()),
                Some(SocketOutcome::Closed) => Err(std::io::Error::other("closed")),
                Some(SocketOutcome::Idle) | None => {
                    Err(std::io::Error::from(std::io::ErrorKind::WouldBlock))
                }
            }
        }
    }

    /// Wait up to `timeout` for `handle` to finish. A bare `.join()` on a
    /// wedged watcher thread would hang the whole suite with no diagnostic
    /// (the same failure mode issue #53 names for socket tests); this fails
    /// loudly instead.
    fn join_with_timeout<T: Send + 'static>(
        handle: std::thread::JoinHandle<T>,
        timeout: Duration,
    ) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(handle.join());
        });
        match rx.recv_timeout(timeout) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => std::panic::resume_unwind(e),
            Err(_) => panic!("watcher thread did not finish within {timeout:?}"),
        }
    }

    const RECV_TIMEOUT: Duration = Duration::from_secs(2);
    const JOIN_TIMEOUT: Duration = Duration::from_secs(5);

    #[test]
    fn watch_pushes_an_initial_snapshot_then_a_debounced_refresh_for_a_burst_of_events() {
        // A near-infinite slow poll isolates this test to the debounce path
        // (own `last_send` bookkeeping) rather than the safety net.
        let timings = Timings {
            slow_ms: 1_000_000,
            debounce_ms: 50,
            focus_ms: 50,
        };
        let socket = ScriptedSocket::new(vec![
            SocketOutcome::Line(r#"{"event":"pane_created"}"#),
            SocketOutcome::Line(r#"{"event":"pane_created"}"#),
        ]);
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = watch(
            cli_feed(),
            Some(Box::new(socket)),
            Box::new(StepClock::new(20)),
            tx,
            timings,
        );

        let initial = rx
            .recv_timeout(RECV_TIMEOUT)
            .expect("the initial snapshot is pushed before any socket event");
        assert_eq!(initial.len(), 1);

        let refreshed = rx
            .recv_timeout(RECV_TIMEOUT)
            .expect("a debounced refresh follows the burst of structural events");
        assert_eq!(refreshed.len(), 1);

        drop(rx);
        join_with_timeout(handle, JOIN_TIMEOUT);
    }

    #[test]
    fn watch_treats_idle_ticks_as_a_normal_wait_and_the_slow_poll_net_still_fires() {
        // The debounce window is effectively infinite, so with zero events at
        // all the only thing that can produce a second snapshot is the
        // slow-poll safety net — proving idle ticks (`WouldBlock`) are neither
        // mistaken for a close nor themselves trigger a refresh.
        let timings = Timings {
            slow_ms: 40,
            debounce_ms: 1_000_000,
            focus_ms: 1_000_000,
        };
        let socket = ScriptedSocket::new(vec![SocketOutcome::Idle; 50]);
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = watch(
            cli_feed(),
            Some(Box::new(socket)),
            Box::new(StepClock::new(10)),
            tx,
            timings,
        );

        let initial = rx.recv_timeout(RECV_TIMEOUT).expect("initial snapshot");
        assert_eq!(initial.len(), 1);

        let polled = rx
            .recv_timeout(RECV_TIMEOUT)
            .expect("the slow-poll net still refreshes with no socket events at all");
        assert_eq!(polled.len(), 1);

        drop(rx);
        join_with_timeout(handle, JOIN_TIMEOUT);
    }

    #[test]
    fn watch_degrades_to_poll_only_when_the_socket_closes_for_real() {
        let timings = Timings {
            slow_ms: 5,
            debounce_ms: 1_000_000,
            focus_ms: 1_000_000,
        };
        let socket = ScriptedSocket::new(vec![SocketOutcome::Closed]);
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = watch(
            cli_feed(),
            Some(Box::new(socket)),
            Box::new(StepClock::new(10)),
            tx,
            timings,
        );

        let initial = rx.recv_timeout(RECV_TIMEOUT).expect("initial snapshot");
        assert_eq!(initial.len(), 1);

        // The socket closes on the very first read; production must degrade
        // to poll-only rather than treat that as fatal, so a refresh must
        // still arrive off the slow-poll safety net alone.
        let polled = rx
            .recv_timeout(RECV_TIMEOUT)
            .expect("a poll-only refresh still arrives after the socket closes");
        assert_eq!(polled.len(), 1);

        drop(rx);
        join_with_timeout(handle, JOIN_TIMEOUT);
    }
}
