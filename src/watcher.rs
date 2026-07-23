//! Background watcher: subscribe to socket events; on any event, debounced-
//! refetch `herdr agent list` and push a snapshot. A slow interval refetch is
//! the safety net; socket failure degrades to poll-only. All timing is behind a
//! clock seam so the debounce/coalesce logic is unit-testable without threads.

use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

use crate::agent::{Agent, parse_agent_list};
use crate::herdr::HerdrCli;
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

/// Fetch a fresh snapshot via the CLI; `None` on any spawn/parse failure so
/// callers can silently skip a bad refetch rather than propagate the error.
fn refetch(cli: &dyn HerdrCli) -> Option<Vec<Agent>> {
    cli.run_json(&["agent", "list"])
        .ok()
        .and_then(|s| parse_agent_list(&s).ok())
}

/// Test seam: consume all events a socket yields, applying the debounce rule,
/// and return the snapshots that would be pushed. `event_time(i)` supplies the
/// clock reading (ms) for the i-th event so tests can place events in/out of
/// the debounce window.
pub fn drain_events(
    cli: &dyn HerdrCli,
    socket: &mut dyn SocketClient,
    debounce_ms: u64,
    mut event_time: impl FnMut(usize) -> u64,
) -> Vec<Vec<Agent>> {
    let _ = socket.send_line(&subscribe_request());
    let mut snaps = Vec::new();
    let mut last_fetch: Option<u64> = None;
    let mut i = 0;
    while let Ok(_line) = socket.recv_line() {
        let t = event_time(i);
        i += 1;
        let due = match last_fetch {
            Some(prev) => t.saturating_sub(prev) >= debounce_ms,
            None => true,
        };
        if due {
            if let Some(s) = refetch(cli) {
                snaps.push(s);
            }
            last_fetch = Some(t);
        }
    }
    snaps
}

/// Spawn the real watcher thread. Pushes an initial snapshot, then subscribes
/// to socket events and pushes a debounced refetch on each burst. If the
/// socket is absent or errors, degrades to polling `herdr agent list` every
/// `slow_ms`; never panics.
pub fn watch(
    cli: Box<dyn HerdrCli + Send>,
    mut socket: Option<Box<dyn SocketClient + Send>>,
    clock: Box<dyn Clock + Send>,
    tx: Sender<Vec<Agent>>,
    slow_ms: u64,
    debounce_ms: u64,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        use std::io::ErrorKind;

        // Initial snapshot.
        if let Some(s) = refetch(cli.as_ref()) {
            let _ = tx.send(s);
        }
        let mut last_send = clock.now_ms();
        if let Some(sock) = socket.as_mut() {
            let _ = sock.send_line(&subscribe_request());
        }
        loop {
            if let Some(sock) = socket.as_mut() {
                match sock.recv_line() {
                    Ok(_line) => {
                        // An event arrived; debounce a refetch.
                        let now = clock.now_ms();
                        if now.saturating_sub(last_send) >= debounce_ms {
                            if let Some(s) = refetch(cli.as_ref())
                                && tx.send(s).is_err()
                            {
                                return;
                            }
                            last_send = now;
                        }
                    }
                    Err(ref e)
                        if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
                    {
                        // Idle tick within the read timeout; not a failure.
                    }
                    Err(_) => {
                        socket = None; // real close -> degrade to poll-only
                    }
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(slow_ms));
            }

            // Slow-poll safety net: guarantees a refresh at least every
            // `slow_ms` regardless of socket state (connected, idle-ticking,
            // or degraded to `None`).
            let now = clock.now_ms();
            if now.saturating_sub(last_send) >= slow_ms {
                if let Some(s) = refetch(cli.as_ref())
                    && tx.send(s).is_err()
                {
                    return;
                }
                last_send = now;
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

    // A fake socket that emits N event lines then blocks forever (returns Err).
    struct FakeSocket {
        remaining: usize,
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
            Ok(r#"{"event":"agent.status_changed"}"#.into())
        }
    }

    #[test]
    fn debounce_coalesces_a_burst_into_one_refetch() {
        let cli = LiveHerdr::with_runner("herdr", FakeRunner);
        // 5 events arriving within the debounce window => 1 snapshot.
        let snaps = drain_events(&cli, &mut FakeSocket { remaining: 5 }, 250, |_| {
            0 /* all same tick */
        });
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].len(), 1);
        assert_eq!(snaps[0][0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn separated_events_produce_separate_refetches() {
        let cli = LiveHerdr::with_runner("herdr", FakeRunner);
        let mut tick = 0u64;
        let snaps = drain_events(&cli, &mut FakeSocket { remaining: 3 }, 250, move |_| {
            tick += 1000;
            tick
        });
        assert_eq!(snaps.len(), 3);
    }
}
